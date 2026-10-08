// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Telechargement BitTorrent reel a travers un circuit dont la SORTIE
//! est un `TunnelCommunity` pyipv8 (`PEER_FLAG_EXIT_BT`) — jalon de
//! fermeture de l'etape 12 : `rqbit -> circuit -> sortie pyipv8 ->
//! seeder`.
//!
//! Topologie : downloader (Rust) -> relais (Rust) -> noeud Python
//! (relais+exit) -> socket uTP du seeder rqbit. Les datagrammes uTP
//! du moteur partent en cellules `data` avec destination reelle
//! (`udp_relay::dial_to`) ; l'exit pyipv8 les relaie au seeder et
//! achemine les reponses en retour.
//!
//! Modes :
//! - defaut : le pair du seeder est injecte via `initial_peers` (pont
//!   `udp_relay::dial_to`).
//! - `--dht` : le downloader decouvre le seeder par la **DHT routee
//!   dans le tunnel** (`TunnelUdpSocket` pour `dht_socket` +
//!   `utp_socket`) — un noeud DHT local (peer_id aligne sur
//!   l'infohash pour passer `min_distance_to_announce`) sert de
//!   bootstrap ; le seeder s'y annonce, le downloader y fait
//!   `get_peers` a travers la sortie pyipv8.
//!
//! Usage :
//!   exit_download_interop --keyfile F --hops N [--dht] [--payload N] [--log L]
//! `--keyfile` contient `<pubkey_hex> <port>` ecrit par
//! `py_tunnel_node.py`. `--hops 1` supprime les relais Rust (circuit
//! direct vers la sortie Python) ; `--hops 3` ajoute deux relais Rust.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::PEER_FLAG_RELAY;
use onionbit_tunnel::tunnel_udp_socket::TunnelUdpSockets;
use onionbit_tunnel::udp_relay;

/// Delai max de telechargement a travers le tunnel.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
/// Taille du fichier telecharge.
const DEFAULT_PAYLOAD: usize = 200_000;

/// Noeud tunnel de test (endpoint + annuaire + community).
struct TunnelNode {
    key: LibNaClSecretKey,
    network: Arc<Network>,
    tunnel: Arc<TunnelCommunity>,
    endpoint: Arc<UdpEndpoint>,
    /// Discovery overlay optionnel (introduction vers un pair externe
    /// comme Tribler.exe — fait de nous un pair verifie dans son
    /// `Network`, ce qui lui evite le `dht_peer_lookup` bloquant
    /// quand il relaie un `extend` vers ce relais).
    discovery: Option<Arc<onionbit_ipv8::discovery::DiscoveryCommunity>>,
    addr: SocketAddr,
}

async fn make_node(
    community_id: onionbit_ipv8::CommunityId,
    next_hop_timeout: Duration,
    with_discovery: bool,
) -> TunnelNode {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let tunnel = TunnelCommunity::new_with_id(
        key.clone(),
        network.clone(),
        ep.clone(),
        onionbit_tunnel::settings::TunnelSettings {
            peer_flags: PEER_FLAG_RELAY,
            next_hop_timeout,
            ..Default::default()
        },
        community_id,
    )
    .await;
    // `my_lan` = adresse UDP reelle du noeud : pyipv8 fait de
    // `source_lan_address` l'adresse du pair (`peer.address`), c'est
    // la ou le relais externe enverra son CREATE.
    let discovery = if with_discovery {
        Some(
            onionbit_ipv8::discovery::DiscoveryCommunity::new(
                key.clone(),
                network.clone(),
                ep.clone(),
                UdpAddress::from(addr),
            )
            .await,
        )
    } else {
        None
    };
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    TunnelNode {
        key,
        network,
        tunnel,
        endpoint: ep,
        discovery,
        addr,
    }
}

/// Logue chaque datagramme UDP du noeud (prefixe + en-tete cellule) —
/// diagnostic interop : permet de distinguer "le relais externe n'a
/// rien envoye" de "recu mais rejete avant dispatch".
fn spawn_tap(node: &TunnelNode, tag: &str) {
    let tag = tag.to_string();
    let ep = node.endpoint.clone();
    tokio::spawn(async move {
        let mut rx = ep.set_tap().await;
        while let Ok((dir, src, data)) = rx.recv().await {
            let head = hex::encode(&data[..data.len().min(30)]);
            eprintln!("TAP[{tag}] {dir:?} {src} len={} head={head}", data.len());
        }
    });
}

fn peer_of(node: &TunnelNode) -> Peer {
    Peer::new(
        node.key.public_key().to_bin(),
        Some(UdpAddress::from(node.addr)),
    )
    .unwrap()
}

/// Port UDP loopback libre (fenetre de course acceptable en test).
fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            "onionbit_tunnel=trace,onionbit_bittorrent=info,librqbit=debug,librqbit_dht=info",
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let mut keyfile = None;
    let mut hops = 2usize;
    let mut payload_len = DEFAULT_PAYLOAD;
    let mut use_dht = false;
    let mut relay_keyfile = None;
    let mut tribler_id = false;
    let mut hop_timeout_ms = 10_000u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--keyfile" => keyfile = args.next(),
            "--hops" => hops = args.next().unwrap().parse().unwrap(),
            "--payload" => payload_len = args.next().unwrap().parse().unwrap(),
            "--dht" => use_dht = true,
            // Prefixe de `TriblerTunnelCommunity` (`a3591a6b…`) au lieu
            // de celui du `TunnelCommunity` pyipv8 generique — a coupler
            // avec `--relay` (Tribler.exe) et `--community-id` cote
            // noeud Python pour que la chaine partage le meme prefixe.
            "--tribler-id" => tribler_id = true,
            // Timeout `next_hop` (ms) : les relais Tribler reels font un
            // `dht_peer_lookup` avant de forwarder un `extend` vers un
            // pair inconnu — peut depasser les 10 s par defaut.
            "--hop-timeout-ms" => hop_timeout_ms = args.next().unwrap().parse().unwrap(),
            // Fichier "pubkey_hex port" d'un pair externe (ex. le vrai
            // Tribler.exe installe) utilise comme PREMIER saut du
            // circuit a la place d'un relais Rust interne.
            "--relay" => relay_keyfile = args.next(),
            _ => {}
        }
    }
    let keyfile = keyfile.expect("--keyfile requis");
    assert!((1..=3).contains(&hops), "--hops doit etre 1..=3");
    if relay_keyfile.is_some() {
        assert!(
            hops >= 2,
            "--relay exige --hops >= 2 (le relais n'est pas la sortie)"
        );
    }

    // Cle publique + port du noeud Python (relais + sortie EXIT_BT).
    let kf = std::fs::read_to_string(&keyfile).expect("lecture keyfile");
    let mut parts = kf.split_whitespace();
    let py_pk = hex::decode(parts.next().expect("pubkey hex")).expect("hex invalide");
    let py_port: u16 = parts.next().expect("port").parse().unwrap();
    let py_peer = Peer::new(
        py_pk.clone(),
        Some(UdpAddress::from(
            format!("127.0.0.1:{py_port}")
                .parse::<SocketAddr>()
                .unwrap(),
        )),
    )
    .expect("cle publique pyipv8 invalide");

    // 1. Torrent reel dans le dossier du seeder (cree avant la DHT :
    //    le peer_id du noeud bootstrap est aligne sur l'infohash).
    let seed_dir = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..payload_len as u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(seed_dir.path().join("fichier.bin"), &payload).unwrap();
    let torrent = librqbit::create_torrent(
        seed_dir.path(),
        librqbit::CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .expect("create_torrent");
    let torrent_bytes = torrent.as_bytes().unwrap();

    // 2. Noeud DHT local (mode --dht) : peer_id = infohash — distance
    //    nulle, l'announce du seeder passe toujours le seuil
    //    `min_distance_to_announce` (sinon aleatoire a ~2^-16). Le
    //    worker DHT exige un bootstrap reussi : on bind la socket
    //    nous-memes pour bootstrapper le noeud sur sa propre adresse
    //    (isole, pas de routeur public).
    let (dht_state, dht_addr) = if use_dht {
        let ih = torrent.info_hash();
        let socket = librqbit_dualstack_sockets::UdpSocket::bind_udp(
            "127.0.0.1:0".parse().unwrap(),
            librqbit_dualstack_sockets::BindOpts {
                request_dualstack: false,
                ..Default::default()
            },
        )
        .expect("bind dht node");
        let addr = socket.bind_addr();
        let dht = librqbit::dht::DhtState::with_config(librqbit::dht::DhtConfig {
            peer_id: Some(librqbit::dht::Id20::new(ih.0)),
            socket: Some(std::sync::Arc::new(socket)),
            bootstrap_addrs: Some(vec![addr.to_string()]),
            ..Default::default()
        })
        .await
        .expect("dht node");
        eprintln!("dht node sur {addr} (peer_id = infohash)");
        // Sonde : les peers annonces pour cet infohash (verifie que
        // l'announce du seeder atteint le noeud).
        {
            use futures_util::StreamExt;
            let dht2 = dht.clone();
            tokio::spawn(async move {
                let mut s = dht2.get_peers(ih, None);
                while let Some(peer) = s.next().await {
                    eprintln!("DHT: peer vu pour l'infohash -> {peer}");
                }
            });
        }
        (Some(dht), Some(addr))
    } else {
        (None, None)
    };

    // 3. Seeder rqbit (uTP uniquement ; en mode --dht il s'annonce
    //    sur le noeud DHT local avec `announce_port` force — librqbit
    //    n'annonce jamais en loopback sinon).
    let seeder_listen_port = free_udp_port();
    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.utp_only = true;
    seed_cfg.listen_port = Some(seeder_listen_port);
    seed_cfg.listen_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    if let Some(dht_addr) = dht_addr {
        seed_cfg.enable_dht = true;
        seed_cfg.dht_bootstrap_addrs = Some(vec![dht_addr.to_string()]);
        seed_cfg.announce_port = Some(seeder_listen_port);
    }
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seeder_listen = SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), seeder_listen_port);

    // 4. Noeuds tunnel : downloader + `hops-1` relais Rust, sortie =
    //    noeud Python (`required_exit`).
    let community_id = if tribler_id {
        onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID
    } else {
        onionbit_tunnel::TUNNEL_COMMUNITY_ID
    };

    // Pair relais externe optionnel (ex. Tribler.exe installe) : meme
    // format de fichier que la keyfile Python (`pubkey_hex port`).
    let ext_relay = relay_keyfile.map(|kf| {
        let s = std::fs::read_to_string(&kf).expect("lecture relay keyfile");
        let mut p = s.split_whitespace();
        let pk = hex::decode(p.next().expect("pubkey hex")).expect("hex invalide");
        let port: u16 = p.next().expect("port").parse().unwrap();
        Peer::new(
            pk,
            Some(UdpAddress::from(
                format!("127.0.0.1:{port}").parse::<SocketAddr>().unwrap(),
            )),
        )
        .expect("cle publique relais invalide")
    });

    let hop_timeout = Duration::from_millis(hop_timeout_ms);
    let d = make_node(community_id, hop_timeout, false).await;
    // `hops` compte la sortie : relais intermediaires = hops-1, dont
    // le premier peut etre le pair externe (`--relay`), le reste des
    // relais Rust internes. Les relais Rust situes derriere un relais
    // externe (Tribler.exe) portent une DiscoveryCommunity : ils se
    // presentent a Tribler par introduction-request pour devenir des
    // pairs verifies dans son annuaire, sinon `on_extend` cote Tribler
    // fait un `dht_peer_lookup` synchrone qui peut depasser le
    // `hop_timeout`.
    let rust_relays = hops - 1 - usize::from(ext_relay.is_some());
    let mut relays = Vec::new();
    let tap_enabled = std::env::args().any(|a| a == "--tap");
    for _ in 0..rust_relays {
        relays.push(make_node(community_id, hop_timeout, ext_relay.is_some()).await);
    }
    if tap_enabled {
        spawn_tap(&d, "dl");
        for r in &relays {
            spawn_tap(r, "rel");
        }
    }
    // Introduction des relais Rust aupres du relais externe (Tribler) :
    // signature verifiee -> `add_verified_peer` cote Tribler.
    if let Some(rp) = &ext_relay {
        if let Some(sa) = rp.address.as_ref().and_then(|a| a.to_socket_addr()) {
            let taddr = UdpAddress::from(sa);
            for r in &relays {
                if let Some(disc) = &r.discovery {
                    for _ in 0..8 {
                        let _ = disc.walk_to(&taddr).await;
                        if disc.intro_response_count() > 0 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    eprintln!(
                        "relais {} : {} intro-response(s) du relais externe",
                        r.addr,
                        disc.intro_response_count()
                    );
                }
            }
        }
    }
    d.network.add_verified(py_peer.clone());
    if let Some(rp) = &ext_relay {
        d.network.add_verified(rp.clone());
        let sa = rp
            .address
            .as_ref()
            .and_then(|a| a.to_socket_addr())
            .expect("adresse relais");
        d.tunnel
            .register_exit_peer(&rp.public_key_bin, sa, PEER_FLAG_RELAY);
    }
    for r in &relays {
        d.network.add_verified(peer_of(r));
        // Registre le relais avec son flag pour que `send_extend` le
        // trouve via `get_candidates_subset([EXIT_BT, RELAY])`.
        d.tunnel
            .register_exit_peer(&r.key.public_key().to_bin(), r.addr, PEER_FLAG_RELAY);
    }
    let first_hop = ext_relay
        .clone()
        .or_else(|| relays.first().map(peer_of))
        .unwrap_or_else(|| py_peer.clone());
    // Sauts intermediaires epingles = relais Rust situes APRES le
    // premier saut : `send_extend` les impose (avec adresse reelle) au
    // lieu de piocher dans les `candidates` annonces par le saut
    // precedent — indispensable quand le premier saut est Tribler.exe
    // connecte au reseau reel (ses candidates sont des pairs publics,
    // qui ne peuvent pas joindre notre sortie loopback).
    let pinned: Vec<Peer> = relays
        .iter()
        .skip(usize::from(ext_relay.is_none()))
        .map(peer_of)
        .collect();
    let cid = if pinned.is_empty() {
        d.tunnel
            .create_circuit_typed(
                hops,
                &first_hop,
                onionbit_tunnel::routing::CIRCUIT_TYPE_DATA,
                Some(py_pk.clone()),
                None,
            )
            .await
    } else {
        d.tunnel
            .create_circuit_pinned(
                hops,
                &first_hop,
                onionbit_tunnel::routing::CIRCUIT_TYPE_DATA,
                Some(py_pk.clone()),
                None,
                pinned,
            )
            .await
    }
    .expect("create_circuit");
    // Un relais Tribler peut faire un `dht_peer_lookup` avant de
    // relayer un `extend` : le ready-wait doit couvrir `hop_timeout`
    // par saut restant, plus une marge.
    let ready_timeout_ms = hop_timeout_ms * (hops as u64 + 1);
    if d.tunnel
        .wait_circuit_ready(cid, ready_timeout_ms)
        .await
        .is_err()
    {
        eprintln!("ECHEC : circuit {cid} jamais READY");
        std::process::exit(1);
    }
    // Route effectivement construite : mids hex des sauts verifies vs
    // attendus (relais externe, relais Rust epingles, sortie pyipv8) —
    // evite qu'un EXTENDED valide sur une route detournee passe pour
    // un succes de la topologie visee.
    let expected: Vec<Peer> = {
        let mut v = Vec::with_capacity(hops);
        if let Some(rp) = &ext_relay {
            v.push(rp.clone());
        }
        v.extend(relays.iter().map(peer_of));
        v.push(py_peer.clone());
        v
    };
    let info = d
        .tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id == cid);
    if let Some(info) = &info {
        let got: Vec<String> = info.verified_hops.clone();
        let want: Vec<String> = expected
            .iter()
            .map(|p| hex::encode(onionbit_crypto::hash::ipv8_mid(&p.public_key_bin)))
            .collect();
        eprintln!("route   = {:?}", got);
        eprintln!("attendu = {:?}", want);
        assert_eq!(
            got, want,
            "route construite differente de la topologie visee"
        );
    }
    eprintln!("circuit {cid} READY ({hops} saut(s), sortie = pyipv8)");

    // 5. Downloader rqbit. Deux chemins d'entree du pair :
    //    - defaut : `initial_peers` = adresse du pont `dial_to` ;
    //    - --dht : decouverte par la DHT routee dans le tunnel
    //      (`TunnelUdpSockets` injectees, aucun pair initial).
    let dl_dir = tempfile::tempdir().unwrap();
    let mut dl_cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
    dl_cfg.utp_only = true;
    dl_cfg.listen_port = Some(0);

    let mut initial_peers = None;
    let _sockets = if use_dht {
        let socks = TunnelUdpSockets::new(d.tunnel.clone(), hops, "127.0.0.1:0".parse().unwrap())
            .expect("tunnel udp sockets");
        dl_cfg.utp_socket = Some(socks.utp.clone());
        dl_cfg.dht_socket = Some(Arc::new(socks.dht.clone()));
        dl_cfg.enable_dht = true;
        dl_cfg.dht_bootstrap_addrs = Some(vec![dht_addr.unwrap().to_string()]);
        Some(socks)
    } else {
        // Pont : la socket `dial` est l'adresse du "pair" pour rqbit ;
        // les datagrammes partent en cellules `data` destinees a la
        // socket uTP du seeder a travers la sortie Python.
        let peer_addr = udp_relay::dial_to(d.tunnel.clone(), cid, UdpAddress::from(seeder_listen))
            .await
            .expect("dial_to relay");
        initial_peers = Some(vec![peer_addr]);
        None
    };

    let downloader = BtEngine::start(dl_cfg).await.expect("downloader engine");
    let dl = downloader
        .add_with_options(
            librqbit::AddTorrent::from_bytes(torrent_bytes),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                initial_peers,
                ..Default::default()
            }),
        )
        .await
        .expect("add download");

    tokio::time::timeout(DOWNLOAD_TIMEOUT, dl.wait_completed())
        .await
        .expect("telechargement via sortie pyipv8 en timeout")
        .expect("wait_completed");

    let got = std::fs::read(dl_dir.path().join("fichier.bin")).unwrap();
    // Tailles demandee (argument) et effectivement verifiee (fichier
    // relu) : les afficher separement evite qu'un succes sous-
    // dimensionne passe pour un gros transfert.
    assert_eq!(
        got.len(),
        payload_len,
        "taille telechargee differente de la taille demandee"
    );
    assert_eq!(got, payload, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
    drop(dht_state);
    eprintln!(
        "INTEROP EXIT DOWNLOAD OK (demande={} octets, verifie={} octets, {} saut(s){})",
        payload_len,
        got.len(),
        hops,
        if use_dht { ", DHT" } else { "" }
    );
}
