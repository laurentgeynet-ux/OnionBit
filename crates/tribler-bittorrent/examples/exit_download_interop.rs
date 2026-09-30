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

use tribler_bittorrent::config::EngineConfig;
use tribler_bittorrent::engine::BtEngine;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::routing::PEER_FLAG_RELAY;
use tribler_tunnel::tunnel_udp_socket::TunnelUdpSockets;
use tribler_tunnel::udp_relay;

/// Delai max de telechargement a travers le tunnel.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
/// Delai max pour que le circuit devienne READY.
const READY_TIMEOUT_MS: u64 = 15_000;
/// Taille du fichier telecharge.
const DEFAULT_PAYLOAD: usize = 200_000;

/// Noeud tunnel de test (endpoint + annuaire + community).
struct TunnelNode {
    key: LibNaClSecretKey,
    network: Arc<Network>,
    tunnel: Arc<TunnelCommunity>,
    addr: SocketAddr,
}

async fn make_node() -> TunnelNode {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let tunnel =
        TunnelCommunity::new(key.clone(), network.clone(), ep.clone(), PEER_FLAG_RELAY).await;
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    TunnelNode {
        key,
        network,
        tunnel,
        addr,
    }
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
        .with_env_filter("tribler_tunnel=trace,tribler_bittorrent=info,librqbit=debug,librqbit_dht=info")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let mut keyfile = None;
    let mut hops = 2usize;
    let mut payload_len = DEFAULT_PAYLOAD;
    let mut use_dht = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--keyfile" => keyfile = args.next(),
            "--hops" => hops = args.next().unwrap().parse().unwrap(),
            "--payload" => payload_len = args.next().unwrap().parse().unwrap(),
            "--dht" => use_dht = true,
            _ => {}
        }
    }
    let keyfile = keyfile.expect("--keyfile requis");
    assert!((1..=3).contains(&hops), "--hops doit etre 1..=3");

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
    let d = make_node().await;
    let mut relays = Vec::new();
    for _ in 1..hops {
        relays.push(make_node().await);
    }
    d.network.add_verified(py_peer.clone());
    for r in &relays {
        d.network.add_verified(peer_of(r));
        // Registre le relais avec son flag pour que `send_extend` le
        // trouve via `get_candidates_subset([EXIT_BT, RELAY])`.
        d.tunnel.register_exit_peer(
            &r.key.public_key().to_bin(),
            r.addr,
            PEER_FLAG_RELAY,
        );
    }
    let first_hop = relays.first().map(peer_of).unwrap_or_else(|| py_peer.clone());
    let cid = d
        .tunnel
        .create_circuit_typed(
            hops,
            &first_hop,
            tribler_tunnel::routing::CIRCUIT_TYPE_DATA,
            Some(py_pk),
            None,
        )
        .await
        .expect("create_circuit");
    if d.tunnel
        .wait_circuit_ready(cid, READY_TIMEOUT_MS)
        .await
        .is_err()
    {
        eprintln!("ECHEC : circuit {cid} jamais READY");
        std::process::exit(1);
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
        let socks =
            TunnelUdpSockets::new(d.tunnel.clone(), hops, "127.0.0.1:0".parse().unwrap())
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
    assert_eq!(got, payload, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
    drop(dht_state);
    eprintln!(
        "INTEROP EXIT DOWNLOAD OK ({} octets, {} saut(s){})",
        payload.len(),
        hops,
        if use_dht { ", DHT" } else { "" }
    );
}
