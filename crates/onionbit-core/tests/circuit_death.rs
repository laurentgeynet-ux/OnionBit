// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rupture de circuit en plein trafic (etapes 13/16 — scenario
//! distinct du test `kill_switch_midtransfer` de `onionbit-bittorrent`
//! qui tue le **proxy**).
//!
//! Ici le listener SOCKS5 de la lane reste joignable en TCP pendant
//! que le seul circuit `READY` est detruit par un vrai `DESTROY` cell
//! du relais. Le test prouve que :
//!
//! 1. des donnees transitent reellement par le tunnel (echo UDP via
//!    `UDP ASSOCIATE` -> cellules `data` -> sortie relais) ;
//! 2. la lane est fail-closed des sa creation (portee `"circuits"`
//!    engagee avant le premier `READY`) et chaque lane n'evalue que
//!    les circuits a **son** nombre de sauts — une lane 2 sauts reste
//!    engagee pendant qu'un circuit 1 saut sert la lane 1 saut ;
//! 3. `proxy joignable != circuit disponible` : la sonde TCP du
//!    watchdog proxy voit toujours un listener sain alors que la
//!    portee `"circuits"` du kill switch est engagee ;
//! 4. aucune fuite : pendant la fenetre morte, zero datagramme
//!    n'atteint la destination (le serveur SOCKS5 n'a aucun chemin
//!    de sortie direct) ;
//! 5. un nouveau circuit `READY` desarme la portee et le trafic
//!    reprend par le tunnel.
//!
//! **Fenetre morte bornee** : elle commence a l'engagement de la
//! portee `"circuits"` (consecutif au `DESTROY` recu -> `on_destroy`)
//! et finit a son desarmement (nouveau circuit `READY`). Destinations
//! surveillees pendant cette fenetre : le serveur d'echo UDP externe
//! (compteur de datagrammes — le seul point de sortie possible du
//! flux) et le socket client de l'association (aucune reponse
//! encapsulee). Le listener TCP SOCKS5, lui, reste joignable : c'est
//! precisement ce qui distingue ce scenario de la mort du proxy.
//!
//! Hors-ligne : tous les sockets sont en loopback.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use onionbit_core::{CoreConfig, CoreSession, Notifier};
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::{DESTROY_REASON_UNNEEDED, PEER_FLAG_EXIT_BT, PEER_FLAG_RELAY};
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

/// Delai max d'attente d'une transition d'etat.
const WAIT: Duration = Duration::from_secs(10);
/// Intervalle de scrutation.
const POLL: Duration = Duration::from_millis(30);
/// Lane anonyme testee (1 saut = circuit le plus simple).
const HOPS: usize = 1;

/// Noeud relais autonome (simule un pair du reseau d'anonymat).
struct RelayNode {
    key: LibNaClSecretKey,
    network: Arc<Network>,
    tunnel: Arc<TunnelCommunity>,
    addr: SocketAddr,
}

async fn make_relay() -> RelayNode {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let tunnel = TunnelCommunity::new(
        key.clone(),
        network.clone(),
        ep.clone(),
        PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT,
    )
    .await;
    tokio::spawn(async move {
        let _ = ep.run().await;
    });
    RelayNode {
        key,
        network,
        tunnel,
        addr,
    }
}

fn peer_of(node: &RelayNode) -> Peer {
    Peer::new(
        node.key.public_key().to_bin(),
        Some(UdpAddress::from(node.addr)),
    )
    .unwrap()
}

/// Attend qu'un predicat devienne vrai (sondage).
async fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while !f() && Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
    }
    f()
}

/// Poignee de main SOCKS5 + `UDP ASSOCIATE` ; retourne la connexion
/// de controle et l'adresse du relais UDP du proxy. `Err` si le proxy
/// refuse (ex. circuit selection impossible cote CONNECT — l'ASSOCIATE
/// ne selectionne le circuit qu'a la premiere frame).
async fn socks5_udp_associate(
    socks_addr: SocketAddr,
) -> Option<(TcpStream, UdpSocket, SocketAddr)> {
    let mut tcp = TcpStream::connect(socks_addr).await.ok()?;
    tcp.write_all(&[0x05, 0x01, 0x00]).await.ok()?;
    let mut g = [0u8; 2];
    tcp.read_exact(&mut g).await.ok()?;
    if g != [0x05, 0x00] {
        return None;
    }
    tcp.write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .ok()?;
    let mut rep = [0u8; 10];
    tcp.read_exact(&mut rep).await.ok()?;
    if rep[0] != 0x05 || rep[1] != 0x00 || rep[3] != 0x01 {
        return None;
    }
    let relay = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(rep[4], rep[5], rep[6], rep[7])),
        u16::from_be_bytes([rep[8], rep[9]]),
    );
    let client_udp = UdpSocket::bind("127.0.0.1:0").await.ok()?;
    Some((tcp, client_udp, relay))
}

/// Frame SOCKS5 UDP (`RSV(2) FRAG ATYP ADDR PORT DATA`) vers `target`.
fn socks5_udp_frame(target: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8, 0, 0, 0x01];
    match target {
        SocketAddr::V4(a) => frame.extend_from_slice(&a.ip().octets()),
        SocketAddr::V6(_) => unreachable!("loopback v4"),
    }
    frame.extend_from_slice(&target.port().to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Datagramme conforme `DataChecker.could_be_utp` (ST_SYN v1) : la
/// sortie n'accepte que du trafic BT/IPv8 reconnaissable.
fn utp_payload(tag: &[u8]) -> Vec<u8> {
    let mut p = vec![0x41, 0x00];
    p.extend_from_slice(&[0; 18]);
    p.extend_from_slice(tag);
    p
}

/// Tente un aller-retour echo via le tunnel : `true` si la reponse
/// revient encapsulee avant `timeout`.
async fn echo_roundtrip(client_udp: &UdpSocket, relay: SocketAddr, echo: SocketAddr) -> bool {
    let msg = utp_payload(b"circuit-probe");
    client_udp
        .send_to(&socks5_udp_frame(echo, &msg), relay)
        .await
        .expect("send frame");
    let mut buf = [0u8; 512];
    match tokio::time::timeout(Duration::from_secs(2), client_udp.recv_from(&mut buf)).await {
        Ok(Ok((n, _))) => n > 10 && &buf[10..n] == msg.as_slice(),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn circuit_detruit_bloque_la_lane_sans_fuite() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    // Session avec stack IPv8 + anonymat (loopback uniquement).
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start session");
    let stack = session.ipv8().expect("stack ipv8");
    let tunnel = stack.tunnel.clone().expect("tunnel community");
    let engine = stack.anon_engine(HOPS).await.expect("lane anonyme");
    let socks_addr = stack
        .anon_lanes()
        .iter()
        .find(|(h, _)| *h == HOPS)
        .map(|(_, a)| *a)
        .expect("lane 1 saut");
    let ks = engine.kill_switch().expect("kill switch de la lane");

    // Fail-closed au demarrage : la lane est indisponible tant
    // qu'aucun circuit READY n'existe — la portee "circuits" est
    // engagee des la creation, avant le premier circuit.
    assert!(ks.is_engaged(), "lane disponible sans circuit");
    assert!(
        ks.reason().map(|r| r.contains("circuits")).unwrap_or(false),
        "portee circuits attendue au demarrage: {:?}",
        ks.reason()
    );

    // Lane a 2 sauts : elle doit rester indisponible tant que seul
    // un circuit a 1 saut existe — l'evaluation est par lane (nombre
    // de sauts exige), pas globale.
    let engine2 = stack.anon_engine(2).await.expect("lane 2 sauts");
    let ks2 = engine2.kill_switch().expect("kill switch lane 2");

    // Relais d'anonymat autonome, appris des deux cotes.
    let relay = make_relay().await;
    let relay_peer = peer_of(&relay);
    stack.network.add_verified(relay_peer.clone());
    stack
        .network
        .discover_service(&relay_peer.public_key_bin, TUNNEL_COMMUNITY_ID);
    let session_peer = Peer::new(
        hex::decode(stack.public_key_hex()).unwrap(),
        Some(UdpAddress::from(stack.endpoint.local_addr().unwrap())),
    )
    .unwrap();
    relay.network.add_verified(session_peer.clone());
    relay
        .network
        .discover_service(&session_peer.public_key_bin, TUNNEL_COMMUNITY_ID);

    // Serveur d'echo "exterieur" : compte les datagrammes pour prouver
    // l'absence de trafic pendant la fenetre morte.
    let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    let echo_rx = Arc::new(AtomicUsize::new(0));
    {
        let echo_rx = echo_rx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((n, src)) = echo.recv_from(&mut buf).await {
                echo_rx.fetch_add(1, Ordering::SeqCst);
                let _ = echo.send_to(&buf[..n], src).await;
            }
        });
    }

    // --- Phase 1 : circuit actif, donnees echangees ---
    let cid = tunnel
        .create_circuit(HOPS, &relay_peer)
        .await
        .expect("create_circuit");
    assert!(
        wait_for(|| tunnel.ready_circuits().contains(&cid)).await,
        "circuit pas READY"
    );
    // Le watchdog desarme la portee "circuits" une fois le circuit vu.
    assert!(
        wait_for(|| !ks.is_engaged()).await,
        "kill switch encore engage avec un circuit READY: {:?}",
        ks.reason()
    );
    // ... mais la lane a 2 sauts reste engagee : le circuit a 1 saut
    // ne satisfait pas son exigence.
    assert!(
        ks2.is_engaged(),
        "la lane 2 sauts s'est desarmee sur un circuit a 1 saut"
    );

    let (_ctrl, client_udp, relay_udp) = socks5_udp_associate(socks_addr)
        .await
        .expect("UDP ASSOCIATE avec circuit actif");
    assert!(
        echo_roundtrip(&client_udp, relay_udp, echo_addr).await,
        "aucune donnee echangee via le circuit"
    );
    assert!(
        echo_rx.load(Ordering::SeqCst) > 0,
        "le serveur echo n'a rien recu"
    );

    // --- Phase 2 : le relais detruit le circuit (DESTROY cell) ---
    let session_addr = UdpAddress::from(stack.endpoint.local_addr().unwrap());
    relay
        .tunnel
        .send_destroy(&session_addr, cid, DESTROY_REASON_UNNEEDED)
        .await
        .expect("send_destroy");
    assert!(
        wait_for(|| !tunnel.ready_circuits().contains(&cid)).await,
        "circuit non supprime cote initiateur"
    );
    // Le watchdog de circuits engage la portee dediee.
    assert!(
        wait_for(|| {
            ks.is_engaged() && ks.reason().map(|r| r.contains("circuits")).unwrap_or(false)
        })
        .await,
        "portee circuits non engagee: {:?}",
        ks.reason()
    );

    // `proxy joignable != circuit disponible` : le listener SOCKS5
    // accepte toujours les connexions TCP — c'est la selection de
    // circuit qui echoue, pas le proxy.
    TcpStream::connect(socks_addr)
        .await
        .expect("le listener SOCKS5 doit rester joignable");

    // Aucune fuite : des frames envoyees pendant la fenetre morte
    // n'atteignent jamais la destination — le proxy n'a aucun chemin
    // de sortie direct (un fallback direct ferait echo).
    let (_ctrl2, dead_udp, dead_relay) = socks5_udp_associate(socks_addr)
        .await
        .expect("UDP ASSOCIATE reste accepte sans circuit");
    let before = echo_rx.load(Ordering::SeqCst);
    for _ in 0..3 {
        let msg = utp_payload(b"leak-probe");
        dead_udp
            .send_to(&socks5_udp_frame(echo_addr, &msg), dead_relay)
            .await
            .unwrap();
    }
    // Aucune reponse encapsulee ne doit revenir non plus.
    let mut buf = [0u8; 512];
    let leaked_reply = tokio::time::timeout(Duration::from_secs(2), dead_udp.recv_from(&mut buf))
        .await
        .is_ok();
    assert_eq!(
        echo_rx.load(Ordering::SeqCst),
        before,
        "fuite directe : datagrammes recus par l'echo sans circuit"
    );
    assert!(!leaked_reply, "reponse recue alors que le circuit est mort");

    // --- Phase 3 : nouveau circuit -> desarmement + reprise ---
    let cid2 = tunnel
        .create_circuit(HOPS, &relay_peer)
        .await
        .expect("re-create_circuit");
    assert!(
        wait_for(|| tunnel.ready_circuits().contains(&cid2)).await,
        "second circuit pas READY"
    );
    assert!(
        wait_for(|| !ks.is_engaged()).await,
        "kill switch encore engage apres nouveau circuit: {:?}",
        ks.reason()
    );
    // La lane 2 sauts n'a toujours pas de circuit utilisable.
    assert!(
        ks2.is_engaged(),
        "la lane 2 sauts s'est desarmee sans circuit a 2 sauts"
    );

    let (_ctrl3, live_udp, live_relay) = socks5_udp_associate(socks_addr)
        .await
        .expect("UDP ASSOCIATE apres reprise");
    assert!(
        echo_roundtrip(&live_udp, live_relay, echo_addr).await,
        "le trafic ne reprend pas via le nouveau circuit"
    );

    session.stop().await;
}

#[tokio::test]
async fn anon_engine_demarre_proprement_avec_dht_active_sur_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.engine.enable_dht = true;
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start session");
    let stack = session.ipv8().expect("stack ipv8");
    // Les lanes anonymes 1, 2 et 3 doivent pouvoir demarrer sans conflit de DHT ni de port
    for hops in 1..=3 {
        let engine = stack.anon_engine(hops).await;
        assert!(
            engine.is_ok(),
            "echec anon_engine({hops}): {:?}",
            engine.err()
        );
    }
    session.stop().await;
}
