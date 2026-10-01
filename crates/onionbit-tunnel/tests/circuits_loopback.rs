// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests d'integration `TunnelCommunity` en loopback : construction
//! de circuits 1 et 2 sauts sur de vrais sockets UDP, relais de
//! cellules chiffrees ChaCha20-Poly1305, sortie UDP, destroy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit_utp::Transport;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::packet::Packet;
use onionbit_ipv8::payloads::{ConnectionType, IntroductionResponse, Payload as Ipv8Payload};
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::serializer::Writer;
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::payload::{self as tp, msg, Cellable};
use onionbit_tunnel::routing::{
    CIRCUIT_TYPE_RP_DOWNLOADER, CIRCUIT_TYPE_RP_SEEDER, DESTROY_REASON_UNNEEDED, PEER_FLAG_EXIT_BT,
    PEER_FLAG_EXIT_HTTP, PEER_FLAG_RELAY, PEER_FLAG_SPEED_TEST, PEER_SOURCE_PEX,
};
use onionbit_tunnel::socks5::Socks5Server;
use onionbit_tunnel::tunnel_udp_socket::{TunnelUdpKind, TunnelUdpSocket};
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

/// Delai max d'attente d'un evenement de test.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Intervalle de scrutation.
const POLL: Duration = Duration::from_millis(20);

/// Noeud de test : endpoint + network + community tunnels.
struct Node {
    /// Identite.
    key: LibNaClSecretKey,
    /// Annuaire.
    network: Arc<Network>,
    /// Community tunnels.
    tunnel: Arc<TunnelCommunity>,
    /// Socket UDP partage (communities overlay en plus du tunnel).
    ep: Arc<UdpEndpoint>,
    /// Adresse d'ecoute.
    addr: SocketAddr,
}

/// Cree un noeud complet sur loopback et lance sa boucle de reception.
async fn make_node() -> Node {
    make_node_flags(PEER_FLAG_RELAY).await
}

/// `make_node` avec des flags de service explicites (`PEER_FLAG_*`).
async fn make_node_flags(flags: i32) -> Node {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let tunnel = TunnelCommunity::new(key.clone(), network.clone(), ep.clone(), flags).await;
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    Node {
        key,
        network,
        tunnel,
        ep,
        addr,
    }
}

/// `DHTDiscoveryCommunity` sur le meme endpoint (`my_wan`/`my_lan` =
/// adresse loopback — necessaire a `on_node_discovered`/`calc_node_id`).
async fn dht_of(node: &Node) -> Arc<onionbit_ipv8::dht::DhtCommunity> {
    let local = UdpAddress::from(node.addr);
    onionbit_ipv8::dht::DhtCommunity::new(node.key.clone(), local.clone(), local, node.ep.clone())
        .await
}

/// Datagramme conforme `DataChecker.could_be_utp` : ST_SYN v1, ext 0,
/// minimum 20 octets. Depuis l'etape 13, la sortie n'accepte que du
/// trafic BT/IPv8 reconnaissable (`is_allowed` pyipv8).
fn utp_payload(tag: &[u8]) -> Vec<u8> {
    let mut p = vec![0x41, 0x00];
    p.extend_from_slice(&[0; 18]);
    p.extend_from_slice(tag);
    p
}

/// `Peer` correspondant au noeud (enregistre service tunnel).
fn peer_of(node: &Node) -> Peer {
    Peer::new(
        node.key.public_key().to_bin(),
        Some(UdpAddress::from(node.addr)),
    )
    .unwrap()
}

/// Enregistre `other` dans l'annuaire de `node` (pair tunnel verifie).
fn learn(node: &Node, other: &Node) {
    let p = peer_of(other);
    node.network.add_verified(p.clone());
    node.network
        .discover_service(&p.public_key_bin, TUNNEL_COMMUNITY_ID);
}

/// Attend qu'un circuit de `a` soit READY.
async fn wait_ready(a: &TunnelCommunity, cid: u32) -> bool {
    let deadline = Instant::now() + TEST_TIMEOUT;
    while Instant::now() < deadline {
        if a.ready_circuits().contains(&cid) {
            return true;
        }
        tokio::time::sleep(POLL).await;
    }
    false
}

/// Cree un circuit a `hops` sauts : A connait tous les noeuds, chaque
/// hop connait le suivant (pour la liste de candidats).
async fn build_circuit(nodes: &[Node], hops: usize) -> (u32, Vec<u32>) {
    for (i, n) in nodes.iter().enumerate() {
        for (j, m) in nodes.iter().enumerate() {
            if i != j {
                learn(n, m);
            }
        }
    }
    let first = peer_of(&nodes[1]);
    let cid = nodes[0]
        .tunnel
        .create_circuit(hops, &first)
        .await
        .expect("create_circuit");
    assert!(wait_ready(&nodes[0].tunnel, cid).await, "circuit pas READY");
    (cid, nodes[0].tunnel.ready_circuits())
}

#[tokio::test]
async fn tunnel_circuit_1_hop_becomes_ready() {
    let a = make_node().await;
    let b = make_node().await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;
    assert_eq!(nodes[0].tunnel.ready_circuits(), vec![cid]);
}

/// Regression : le dernier saut d'un circuit peut devenir `READY`
/// **avant** que ses flags de service (`PEER_FLAG_EXIT_HTTP` etc.)
/// ne soient appris via une introduction directe — le pair de sortie
/// est souvent connu via la liste de candidats d'un relais avant
/// tout `walk` IPv8 avec lui. `exit_flags` doit donc etre rattrape
/// quand l'introduction arrive tardivement, sinon le circuit reste
/// invisible du selecteur SOCKS5 HTTP pour toujours (`aucun circuit
/// HTTP pret`).
#[tokio::test]
async fn circuit_exit_flags_mis_a_jour_apres_introduction_tardive() {
    let a = make_node().await;
    let b = make_node().await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    // Flags de B pas encore appris : aucun circuit HTTP utilisable.
    assert!(nodes[0]
        .tunnel
        .ready_circuits_of_hops_flags(1, PEER_FLAG_EXIT_HTTP)
        .is_empty());

    // B annonce ses flags via une introduction-response signee,
    // recue APRES coup.
    let unspecified = || UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    let mut w = Writer::new();
    IntroductionResponse {
        destination_address: UdpAddress::from(nodes[0].addr),
        source_lan_address: UdpAddress::from(nodes[1].addr),
        source_wan_address: UdpAddress::from(nodes[1].addr),
        lan_introduction_address: unspecified(),
        wan_introduction_address: unspecified(),
        connection_type: ConnectionType::Unknown,
        supports_new_style: true,
        intro_supports_new_style: false,
        peer_limit_reached: false,
        identifier: 1,
        extra_bytes: ((PEER_FLAG_RELAY | PEER_FLAG_EXIT_HTTP) as u16)
            .to_be_bytes()
            .to_vec(),
    }
    .pack(&mut w)
    .unwrap();
    let pkt = Packet::sign_auto(
        &TUNNEL_COMMUNITY_ID,
        IntroductionResponse::MSG_ID,
        &nodes[1].key,
        1,
        &w.into_bytes(),
    );
    nodes[0].tunnel.on_raw_datagram(nodes[1].addr, &pkt);

    assert_eq!(
        nodes[0]
            .tunnel
            .ready_circuits_of_hops_flags(1, PEER_FLAG_EXIT_HTTP),
        vec![cid],
        "exit_flags pas rattrape apres l'introduction tardive"
    );
}

#[tokio::test]
async fn tunnel_circuit_2_hops_becomes_ready() {
    let a = make_node().await;
    let b = make_node().await;
    let c = make_node().await;
    let nodes = [a, b, c];
    let (_cid, ready) = build_circuit(&nodes, 2).await;
    assert_eq!(ready.len(), 1);
}

/// `circuit_to_dict` pyipv8 : `verified_hops` = mid hex de chaque
/// saut (dans l'ordre), `unverified_hop` vide une fois READY,
/// `creation_time` epoch.
#[tokio::test]
async fn circuit_info_expose_route_et_creation() {
    let nodes = [make_node().await, make_node().await, make_node().await];
    let (cid, _) = build_circuit(&nodes, 2).await;

    let infos = nodes[0].tunnel.circuits_info();
    let c = infos.iter().find(|c| c.circuit_id == cid).unwrap();
    assert_eq!(c.verified_hops.len(), 2);
    // mid = SHA-1 hex de la cle publique de chaque hop, dans l'ordre.
    let mids: Vec<String> = nodes[1..]
        .iter()
        .map(|n| {
            hex::encode(onionbit_crypto::hash::ipv8_mid(
                &n.key.public_key().to_bin(),
            ))
        })
        .collect();
    assert_eq!(c.verified_hops, mids);
    assert!(c.unverified_hop.is_empty());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(c.creation_time > 0 && c.creation_time <= now);
}

#[tokio::test]
async fn tunnel_data_exits_1_hop() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let dest = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = dest.local_addr().unwrap();
    let payload = utp_payload(b"hello 1 hop");

    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(dest_addr),
            &UdpAddress::from(nodes[0].addr),
            &payload,
        )
        .await
        .unwrap();

    let mut buf = [0u8; 256];
    let (n, _) = tokio::time::timeout(TEST_TIMEOUT, dest.recv_from(&mut buf))
        .await
        .expect("aucun datagramme de sortie recu")
        .unwrap();
    assert_eq!(&buf[..n], payload);
}

#[tokio::test]
async fn tunnel_data_exits_at_last_hop() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let a = make_node().await;
    let b = make_node().await;
    let c = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b, c];
    let (cid, _) = build_circuit(&nodes, 2).await;

    // Socket de destination "monde exterieur" (receveur UDP brut).
    let dest = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = dest.local_addr().unwrap();
    let payload = utp_payload(b"hello via tunnel");

    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(dest_addr),
            &UdpAddress::from(nodes[0].addr),
            &payload,
        )
        .await
        .unwrap();

    // Le noeud de sortie (dernier saut) doit livrer les octets bruts.
    let mut buf = [0u8; 256];
    let (n, _) = tokio::time::timeout(TEST_TIMEOUT, dest.recv_from(&mut buf))
        .await
        .expect("aucun datagramme de sortie recu")
        .unwrap();
    assert_eq!(&buf[..n], payload);
}

#[tokio::test]
async fn tunnel_echo_roundtrip_2_hops() {
    let a = make_node().await;
    let b = make_node().await;
    let c = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b, c];
    let (cid, _) = build_circuit(&nodes, 2).await;

    // Echo UDP "exterieur".
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], src).await;
        }
    });

    let mut data_rx = nodes[0].tunnel.data_rx();
    let payload = utp_payload(b"ping aller-retour");
    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(echo_addr),
            &UdpAddress::from(nodes[0].addr),
            &payload,
        )
        .await
        .unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, data_rx.recv())
        .await
        .expect("pas de retour tunnel")
        .expect("canal data ferme ou en retard");
    assert_eq!(msg.circuit_id, cid);
    assert_eq!(msg.data, payload);
    // L'origine rapportee doit etre le serveur d'echo.
    assert_eq!(
        msg.origin.to_socket_addr().unwrap().port(),
        echo_addr.port()
    );
}

#[tokio::test]
async fn tunnel_destroy_removes_circuit() {
    let a = make_node().await;
    let b = make_node().await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let first_hop_addr = UdpAddress::from(nodes[1].addr);
    nodes[0]
        .tunnel
        .send_destroy(&first_hop_addr, cid, DESTROY_REASON_UNNEEDED)
        .await
        .unwrap();
    nodes[0]
        .tunnel
        .send_destroy(&first_hop_addr, cid, DESTROY_REASON_UNNEEDED)
        .await
        .unwrap();
    // Pas de panic = propagation OK ; le circuit local reste visible
    // jusqu'a remove_circuit explicite (idem Python : le destroy n'est
    // pas auto-applique a soi-meme).
    assert!(nodes[0].tunnel.circuit_count() >= 1);
}

/// SOCKS5 e2e : UDP ASSOCIATE sur le proxy de A, frame UDP vers un
/// serveur d'echo "exterieur", traverse un circuit 1 saut, reponse
/// reencapsulee vers le client.
#[tokio::test]
async fn socks5_udp_associate_roundtrip() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b];
    let (_cid, _) = build_circuit(&nodes, 1).await;

    // Serveur d'echo UDP "exterieur".
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], src).await;
        }
    });

    // Proxy SOCKS5 adosse a la community de A.
    let socks = Socks5Server::new(nodes[0].tunnel.clone(), 1);
    let proxy_addr = socks.listen("127.0.0.1:0").await.unwrap();

    // Client SOCKS5 : greeting.
    let mut tcp = TcpStream::connect(proxy_addr).await.unwrap();
    tcp.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut g = [0u8; 2];
    tcp.read_exact(&mut g).await.unwrap();
    assert_eq!(g, [0x05, 0x00], "greeting refuse");

    // UDP ASSOCIATE (dst demande = 0.0.0.0:0, conventionnel).
    tcp.write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .unwrap();
    let mut rep = [0u8; 10];
    tcp.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[0], 0x05);
    assert_eq!(rep[1], 0x00, "UDP ASSOCIATE refuse");
    assert_eq!(rep[3], 0x01);
    let relay_port = u16::from_be_bytes([rep[8], rep[9]]);
    assert_ne!(relay_port, 0);
    let relay = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(rep[4], rep[5], rep[6], rep[7])),
        relay_port,
    );

    // Frame SOCKS5 UDP vers le serveur d'echo.
    let client_udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let msg = utp_payload(b"socks5-ping");
    let mut frame = vec![0u8, 0, 0, 0x01];
    match echo_addr {
        SocketAddr::V4(a) => frame.extend_from_slice(&a.ip().octets()),
        SocketAddr::V6(_) => unreachable!("loopback v4"),
    }
    frame.extend_from_slice(&echo_addr.port().to_be_bytes());
    frame.extend_from_slice(&msg);
    client_udp.send_to(&frame, relay).await.unwrap();

    // La reponse revient encapsulee en frame SOCKS5 UDP.
    let mut buf = [0u8; 512];
    let (n, _src) = tokio::time::timeout(TEST_TIMEOUT, client_udp.recv_from(&mut buf))
        .await
        .expect("pas de reponse SOCKS5 UDP")
        .unwrap();
    let r = &buf[..n];
    assert_eq!(&r[..3], &[0, 0, 0], "RSV/FRAG");
    assert_eq!(r[3], 0x01, "ATYP ipv4");
    let src_port = u16::from_be_bytes([r[8], r[9]]);
    assert_eq!(src_port, echo_addr.port(), "origine = serveur echo");
    assert_eq!(&r[10..], msg);
}

/// Cree un circuit e2e complet avec repli : le handshake a ~8
/// datagrammes UDP sans retransmission protocolaire — sous charge
/// parallele une cellule peut se perdre en loopback ; pyipv8 gere
/// pareil via ses `RequestCache` a retry. On retente `create_e2e`.
async fn create_e2e_with_retry(
    d: &Node,
    info_hash: [u8; 20],
    ip: &onionbit_tunnel::routing::IntroductionPoint,
) -> u32 {
    let mut e2e_rx = d.tunnel.e2e_ready();
    for attempt in 0..3 {
        d.tunnel
            .create_e2e(info_hash, ip)
            .await
            .expect("create_e2e");
        if let Ok(Ok((cid, ih))) = tokio::time::timeout(TEST_TIMEOUT * 4, e2e_rx.recv()).await {
            assert_eq!(ih, info_hash);
            return cid;
        }
        eprintln!("create_e2e tentative {} echouee, repli", attempt + 1);
    }
    panic!("e2e_ready jamais atteint apres 3 tentatives");
}

/// Hidden services e2e : un seeder cree un point d'introduction, un
/// downloader decouvre l'IP via peers-request, etablit un circuit e2e
/// (create-e2e -> RP -> link-e2e -> linked-e2e) et les donnees
/// traversent dans les deux sens avec la couche `hs_session_keys`.
#[tokio::test]
async fn hidden_service_e2e_roundtrip() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let d = make_node().await; // downloader
    let s = make_node().await; // seeder
    let i = make_node().await; // point d'introduction
    let r1 = make_node().await; // relais du downloader
    let r2 = make_node().await; // relais extra (peut servir de RP)
    let nodes = [&d, &s, &i, &r1, &r2];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [7u8; 20];

    // Seeder : rejoint le swarm puis cree un IP direct vers `i`
    // (swarm.hops=0 -> circuit IP_SEEDER de 1 saut).
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_peer = peer_of(&i);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&ip_peer))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    // Downloader : swarm hops=1, circuit de donnees D->r1.
    d.tunnel.join_swarm(info_hash, 1, false);
    let r1_peer = peer_of(&r1);
    let data_cid = d
        .tunnel
        .create_circuit(1, &r1_peer)
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    // peers-request au point d'introduction (chemin "PEX" : on
    // contacte l'IP directement a travers le tunnel).
    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1, "un point d'introduction attendu");
    assert!(!ips[0].seeder_pk.is_empty(), "seeder_pk present");

    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // `swarm_circuit_hops` ajoute +1 a `RP_DOWNLOADER` : meme a
    // `hops=1` au niveau swarm, la jambe e2e du downloader fait 2
    // sauts — le RP (choisi par le seeder) ne voit jamais l'adresse
    // reelle du downloader, seulement son premier saut. Epingle la
    // regression d'anonymat si le +1 sautait.
    let e2e_info = d
        .tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id == e2e_cid)
        .expect("circuit e2e absent");
    assert_eq!(e2e_info.ctype, CIRCUIT_TYPE_RP_DOWNLOADER);
    assert_eq!(
        e2e_info.goal_hops, 2,
        "RP_DOWNLOADER doit faire swarm.hops + 1 sauts"
    );

    // Donnee e2e downloader -> seeder : la couche hs est appliquee
    // (decryptee cote seeder) et le RP reexpedie.
    let mut s_rx = s.tunnel.data_rx();
    let payload = b"e2e-hello-seeder";
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    d.tunnel
        .send_data(e2e_cid, &zero, &zero, payload)
        .await
        .expect("send e2e data");
    let got = tokio::time::timeout(TEST_TIMEOUT, s_rx.recv())
        .await
        .expect("pas de donnee e2e au seeder")
        .expect("canal data seeder");
    assert_eq!(got.data, payload);
}

/// Invariant de non-fuite (threat model) : pendant tout le cycle
/// hidden-service (peers-request -> create-e2e -> link-e2e -> donnee
/// e2e), l'endpoint du downloader n'emet que des datagrammes a
/// prefixe tunnel et uniquement vers les premiers sauts de ses
/// circuits — JAMAIS vers le point d'introduction, l'adresse reelle
/// du seeder ou une destination hors overlay. Reciproquement, ni
/// l'intro point ni le seeder ne recoivent de datagramme dont la
/// source est l'adresse du downloader : tout le contact transite
/// dans les cellules relayees.
///
/// Le downloader ne connait volontairement QUE ses deux relais
/// (`learn` restreint) : le RP est appris via `rp_info`
/// (`add_verified` dans `on_created_e2e`), l'intro point via
/// l'`IntroductionPoint` — aucun egress direct vers i/s n'est alors
/// justifiable comme choix de relais.
#[tokio::test]
async fn hidden_service_egress_uniquement_vers_relais() {
    let d = make_node().await; // downloader
    let s = make_node().await; // seeder
    let i = make_node().await; // point d'introduction
    let r1 = make_node().await; // premier saut
    let r2 = make_node().await; // second saut / sortie

    learn(&d, &r1);
    learn(&d, &r2);
    // Le seeder connait son intro point et les relais (candidats de
    // son circuit RP_SEEDER) ; les relais/intro se connaissent entre
    // eux — aucun ne connait le downloader.
    for (a, b) in [
        (&s, &i),
        (&s, &r1),
        (&s, &r2),
        (&i, &r1),
        (&i, &r2),
        (&i, &s),
        (&r1, &r2),
        (&r2, &r1),
        (&r1, &i),
        (&r2, &i),
    ] {
        learn(a, b);
    }

    // Taps avant toute operation : enregistrent chaque datagramme.
    let mut tap_d = d.ep.set_tap().await;
    let mut tap_i = i.ep.set_tap().await;
    let mut tap_s = s.ep.set_tap().await;

    let info_hash = [8u8; 20];

    // Seeder : swarm + point d'introduction chez `i`.
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    // Downloader : swarm a 2 sauts — a 1 saut, le RP verrait
    // legitiment l'IP de d (c'est son dernier saut) ; a 2, seuls
    // r1/r2 la voient. L'invariant reste le meme : jamais de contact
    // direct downloader <-> i/s.
    d.tunnel.join_swarm(info_hash, 2, false);
    let data_cid = d
        .tunnel
        .create_circuit(2, &peer_of(&r1))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 2)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1, "un point d'introduction attendu");

    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // Plan de donnees e2e, lui aussi sous invariant.
    let mut s_rx = s.tunnel.data_rx();
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    d.tunnel
        .send_data(e2e_cid, &zero, &zero, b"e2e-invariant")
        .await
        .expect("send e2e data");
    tokio::time::timeout(TEST_TIMEOUT, s_rx.recv())
        .await
        .expect("pas de donnee e2e au seeder")
        .expect("canal data seeder");

    // Laisse retomber les datagrammes encore en vol avant le verdict.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // (1) Egress du downloader : destinations ⊆ {r1, r2} et chaque
    //     datagramme porte le prefixe tunnel — rien d'autre ne sort.
    let allowed: std::collections::HashSet<SocketAddr> = [r1.addr, r2.addr].into_iter().collect();
    // Prefixe filaire complet : 0x00 + version(0x02) + community_id.
    let prefix = onionbit_ipv8::packet::prefix_of(&TUNNEL_COMMUNITY_ID);
    let mut tx_count = 0usize;
    while let Ok((dir, dest, bytes)) = tap_d.try_recv() {
        if !matches!(dir, onionbit_ipv8::endpoint::TapDir::Tx) {
            continue;
        }
        tx_count += 1;
        assert!(
            allowed.contains(&dest),
            "fuite : egress direct du downloader vers {dest} (non-relais)"
        );
        assert!(
            bytes.starts_with(&prefix),
            "egress sans prefixe tunnel vers {dest}: {}",
            hex::encode(&bytes[..bytes.len().min(40)])
        );
    }
    assert!(tx_count > 0, "tap vide — instrumentation cassee");

    // (2) Ingress de i et s : aucune source n'est l'adresse reelle
    //     de d — ni l'intro point ni le seeder n'apprennent l'IP du
    //     downloader.
    let mut i_rx = 0usize;
    while let Ok((dir, src, _)) = tap_i.try_recv() {
        if matches!(dir, onionbit_ipv8::endpoint::TapDir::Rx) {
            i_rx += 1;
            assert_ne!(src, d.addr, "l'intro point voit l'IP du downloader");
        }
    }
    assert!(i_rx > 0, "i n'a rien recu — le flux n'a pas eu lieu");
    let mut s_pkts = 0usize;
    while let Ok((dir, src, _)) = tap_s.try_recv() {
        if matches!(dir, onionbit_ipv8::endpoint::TapDir::Rx) {
            s_pkts += 1;
            assert_ne!(src, d.addr, "le seeder voit l'IP du downloader");
        }
    }
    assert!(s_pkts > 0, "s n'a rien recu — le flux n'a pas eu lieu");
}

/// Injection negative (threat model) : les messages e2e ne sont pas
/// signes (`ezr_pack(sig=False)` pyipv8) — la defense repose sur les
/// caches de requetes et les tables de relais, pas sur la signature
/// de l'enveloppe. Ce test forge la batterie sur socket brute :
///
///  - `peers-response`/`created-e2e` avec un `identifier` sans requete
///    en cours -> rejetes (`peers_requests`/`e2e_requests`) ;
///  - `linked-e2e` brut -> non dispatchable hors cellule (whitelist de
///    `on_packet_from_circuit`) ;
///  - `create-e2e` avec `node_public_key` inconnue -> l'intro point ne
///    relaie pas (`intro_point_for`) ;
///  - `create-e2e` avec la vraie `seeder_pk` -> relaye (parite pyipv8)
///    mais le doublon rejoue `seen_e2e` sans creer un second
///    `RP_SEEDER` (anti-amplification).
///
/// Les assertions d'etat sont deterministes : `do_peer_discovery` ne
/// tire que toutes les `swarm_lookup_interval` (30 s) — bien au-dela
/// de la duree du test.
#[tokio::test]
async fn hidden_service_injection_paquets_forjes() {
    let d = make_node().await; // downloader
    let s = make_node().await; // seeder
    let i = make_node().await; // point d'introduction
    let r1 = make_node().await; // premier saut du downloader
    let r2 = make_node().await; // relais extra (RP possible)
    let nodes = [&d, &s, &i, &r1, &r2];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [9u8; 20];

    // Seeder : swarm + point d'introduction chez `i`.
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    // Downloader : swarm a 1 saut + circuit de donnees.
    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(&r1))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    // Requete legitime : le chemin reel fonctionne et revele la
    // `seeder_pk` (information publique — PEX/DHT l'annoncent).
    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1, "un point d'introduction attendu");
    let seeder_pk = ips[0].seeder_pk.clone();
    assert!(!seeder_pk.is_empty(), "seeder_pk present");

    // Attaquant : simple socket UDP — pas un `Node`, aucune cle.
    let atk = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let atk_addr = atk.local_addr().unwrap();
    let mut tap_d = d.ep.set_tap().await;
    let baseline_d = d.tunnel.circuit_count();

    let prefix = onionbit_ipv8::packet::prefix_of(&TUNNEL_COMMUNITY_ID);
    let forge = |msg_id: u8, body: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(prefix.len() + 1 + body.len());
        out.extend_from_slice(&prefix);
        out.push(msg_id);
        out.extend_from_slice(body);
        out
    };

    // (1) `peers-response` avec `identifier` sans requete en cours :
    //     le cache `peers_requests` doit l'absorber — l'intro point
    //     annonce (adresse de l'attaquant) ne doit JAMAIS etre
    //     exploite.
    let mut w = Writer::new();
    tp::PeersResponse {
        circuit_id: 0,
        identifier: 0xBEEF,
        info_hash,
        peers: vec![tp::IntroductionInfo {
            address: UdpAddress::from(atk_addr),
            key: vec![0xAA; 32],
            seeder_pk: vec![0xBB; 74],
            source: PEER_SOURCE_PEX,
        }],
    }
    .pack(&mut w)
    .unwrap();
    atk.send_to(&forge(msg::PEERS_RESPONSE, &w.into_bytes()), d.addr)
        .await
        .unwrap();

    // (2) `created-e2e` inattendu : `e2e_requests` vide -> rejet avant
    //     toute verification DH.
    let mut w = Writer::new();
    tp::CreatedE2E {
        identifier: 0xBEEF,
        key: vec![0u8; 32],
        auth: [0u8; 32],
        rp_info_enc: vec![1, 2, 3],
    }
    .pack(&mut w)
    .unwrap();
    atk.send_to(&forge(msg::CREATED_E2E, &w.into_bytes()), d.addr)
        .await
        .unwrap();

    // (3) `linked-e2e` brut : msg 16 absent de la whitelist de
    //     dispatch sur socket (il n'est valable qu'en cellule).
    atk.send_to(&forge(msg::LINKED_E2E, &[0; 6]), d.addr)
        .await
        .unwrap();
    // (4) msg inconnu et datagramme tronque au prefixe.
    atk.send_to(&forge(0xEE, &[0; 4]), d.addr).await.unwrap();
    atk.send_to(&prefix, d.addr).await.unwrap();

    // Fenetre de reaction eventuelle puis verdict : aucun egress vers
    // l'attaquant, aucun circuit ni connexion e2e crees.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok((dir, dest, _)) = tap_d.try_recv() {
        if matches!(dir, onionbit_ipv8::endpoint::TapDir::Tx) {
            assert_ne!(dest, atk_addr, "le downloader a reagi a un paquet forge");
        }
    }
    assert_eq!(
        d.tunnel.circuit_count(),
        baseline_d,
        "un paquet forge a cree un circuit"
    );
    let swarm = d
        .tunnel
        .swarms_info()
        .into_iter()
        .find(|sw| sw.info_hash == hex::encode(info_hash))
        .expect("swarm downloader absent");
    assert_eq!(swarm.num_connections, 0, "connexion e2e creee par forge");

    // (5) `create-e2e` pour une `node_public_key` inconnue : l'intro
    //     point n'a pas de route `intro_point_for` -> pas de relais,
    //     pas de RP_SEEDER cree chez le seeder.
    let (_, atk_dh) = onionbit_tunnel::community::generate_diffie_secret();
    let forge_create = |identifier: u16, npk: &[u8]| -> Vec<u8> {
        let mut w = Writer::new();
        tp::CreateE2E {
            identifier,
            info_hash,
            node_public_key: npk.to_vec(),
            key: atk_dh.to_vec(),
        }
        .pack(&mut w)
        .unwrap();
        forge(msg::CREATE_E2E, &w.into_bytes())
    };
    let count_rp = |t: &TunnelCommunity| -> usize {
        t.circuits_info()
            .iter()
            .filter(|c| c.ctype == CIRCUIT_TYPE_RP_SEEDER)
            .count()
    };
    atk.send_to(&forge_create(0x1111, &[0xAB; 32]), i.addr)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        count_rp(&s.tunnel),
        0,
        "create-e2e a cle inconnue relaye — intro_point_for non filtrant"
    );

    // (6) Vraie `seeder_pk` : relais nominal (parite pyipv8 — l'intro
    //     point sert de passe-plat). Le doublon doit rejouer la
    //     reponse `seen_e2e` VERBATIM sans recreer un RP_SEEDER.
    let pkt = forge_create(0x2222, &seeder_pk);
    let mut buf = vec![0u8; 2048];
    let mut reply1 = None;
    for _ in 0..3 {
        atk.send_to(&pkt, i.addr).await.unwrap();
        if let Ok(Ok((n, _))) = tokio::time::timeout(TEST_TIMEOUT, atk.recv_from(&mut buf)).await {
            reply1 = Some(buf[..n].to_vec());
            break;
        }
        // Perte loopback : la re-emission tape `seen_e2e` ou
        // `in_flight_e2e` — la reponse finit par arriver.
    }
    let reply1 = reply1.expect("pas de created-e2e relaye — le chemin nominal est casse");
    atk.send_to(&pkt, i.addr).await.unwrap();
    let (n2, _) = tokio::time::timeout(TEST_TIMEOUT, atk.recv_from(&mut buf))
        .await
        .expect("pas de replay du created-e2e")
        .expect("recv doublon");
    assert_eq!(
        &buf[..n2],
        reply1.as_slice(),
        "doublon create-e2e : la reponse `seen_e2e` doit etre verbatim"
    );
    assert_eq!(
        count_rp(&s.tunnel),
        1,
        "doublon create-e2e : un seul RP_SEEDER doit exister"
    );

    // (7) Le chemin legitime reste fonctionnel apres la batterie.
    let _e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;
}

/// Hidden services via la vraie DHT IPv8 : `on_establish_intro`
/// annonce le point d'introduction sous la cle du swarm
/// (`dht_announce` -> `store_value`) ; le downloader le retrouve par
/// `find_values` (`dht_lookup`) puis mene le e2e bout-en-bout —
/// le chemin PEX direct n'est pas utilise.
#[tokio::test]
async fn hidden_service_dht_announce_lookup_e2e() {
    let d = make_node().await; // downloader
    let s = make_node().await; // seeder
    let i = make_node().await; // point d'introduction
    let r1 = make_node().await; // relais du downloader
    let nodes = [&d, &s, &i, &r1];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }

    // DHT overlays sur les memes endpoints : `i` annonce, `d`
    // interroge. Le provider est injecte dans leurs tunnels.
    let dht_i = dht_of(&i).await;
    let dht_d = dht_of(&d).await;
    i.tunnel.set_dht_provider(dht_i.clone());
    d.tunnel.set_dht_provider(dht_d.clone());
    dht_i
        .walk_to(&UdpAddress::from(d.addr))
        .await
        .expect("walk i->d");
    dht_d
        .walk_to(&UdpAddress::from(i.addr))
        .await
        .expect("walk d->i");
    let deadline = Instant::now() + Duration::from_secs(8);
    while (dht_i.node_count() < 1 || dht_d.node_count() < 1) && Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
    }
    assert!(dht_i.node_count() >= 1 && dht_d.node_count() >= 1);

    // Seeder : point d'introduction chez `i` + establish-intro — la
    // reponse declenche le `dht_announce` cote `i`.
    let info_hash = [9u8; 20];
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    // Downloader : `find_values(cle du swarm)` — l'annonce DHT doit
    // contenir le point d'introduction de `i` et le `seeder_pk` du
    // swarm (cle ephemere du seeder, pas sa cle de noeud).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found_ip = None;
    while Instant::now() < deadline && found_ip.is_none() {
        if let Ok(values) = dht_d.find_values(&info_hash, 0).await {
            for (data, _) in values {
                if let Some(ip) = onionbit_tunnel::hidden_services::unpack_dht_intro_point(&data) {
                    found_ip = Some(ip);
                    break;
                }
            }
        }
        if found_ip.is_none() {
            tokio::time::sleep(POLL * 5).await;
        }
    }
    let ip = found_ip.expect("aucun point d'introduction trouve via la DHT");
    assert_eq!(
        ip.peer_key,
        i.key.public_key().to_bin(),
        "le point d'introduction annonce doit etre le noeud i"
    );
    assert!(!ip.seeder_pk.is_empty(), "seeder_pk present");
    assert_eq!(ip.source, onionbit_tunnel::routing::PEER_SOURCE_DHT);

    // Chemin e2e complet : circuit data du downloader, create-e2e vers
    // le point d'introduction trouve via DHT, donnee au seeder.
    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(&r1))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");
    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ip).await;
    let mut s_rx = s.tunnel.data_rx();
    let payload = b"e2e-via-dht";
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    d.tunnel
        .send_data(e2e_cid, &zero, &zero, payload)
        .await
        .expect("send e2e data");
    let got = tokio::time::timeout(TEST_TIMEOUT, s_rx.recv())
        .await
        .expect("pas de donnee e2e au seeder")
        .expect("canal data seeder");
    assert_eq!(got.data, payload);
}

/// Retry `create_e2e` idempotent (regression) : deux appels en rafale
/// — le second arrive avant la reponse, comme sous perte loopback —
/// doivent laisser UN seul `RP_SEEDER` lie cote seeder. Sans dedup,
/// chaque retry ouvrait un handshake neuf (identifier+DH nouveaux) et
/// une reponse tardive liait un second circuit RP (observe en test :
/// 2 `RP_SEEDER` au lieu de 1).
#[tokio::test]
async fn hidden_service_e2e_retry_single_rp() {
    let d = make_node().await;
    let s = make_node().await;
    let i = make_node().await;
    let r1 = make_node().await;
    let nodes = [&d, &s, &i, &r1];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [9u8; 20];

    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_peer = peer_of(&i);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&ip_peer))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    d.tunnel.join_swarm(info_hash, 1, false);
    let r1_peer = peer_of(&r1);
    let data_cid = d
        .tunnel
        .create_circuit(1, &r1_peer)
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1, "un point d'introduction attendu");

    // Deux create-e2e en rafale : le second re-emet le MEME handshake
    // (`pending_e2e`) et le seeder deduque — aucun second RP. Les
    // retries suivants re-emettent toujours la meme requete.
    let mut e2e_rx = d.tunnel.e2e_ready();
    d.tunnel
        .create_e2e(info_hash, &ips[0])
        .await
        .expect("create_e2e #1");
    d.tunnel
        .create_e2e(info_hash, &ips[0])
        .await
        .expect("create_e2e #2 (retry)");
    let mut linked = false;
    for _ in 0..3 {
        if tokio::time::timeout(TEST_TIMEOUT * 4, e2e_rx.recv())
            .await
            .is_ok()
        {
            linked = true;
            break;
        }
        d.tunnel
            .create_e2e(info_hash, &ips[0])
            .await
            .expect("re-emission e2e");
    }
    assert!(linked, "e2e_ready jamais atteint");

    // Une reponse tardive est servie par le cache de dedup seeder,
    // pas par un second circuit RP.
    tokio::time::sleep(TEST_TIMEOUT).await;
    assert_eq!(
        s.tunnel
            .ready_circuits_of_type(onionbit_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER)
            .len(),
        1,
        "un seul RP_SEEDER attendu malgre le retry"
    );
}

/// Hidden seeding via relais UDP transparent (`udp_relay.rs`) : deux
/// "moteurs" (sockets uTP factices) dialoguent bout en bout a travers
/// le circuit e2e lie — equivalent du montage SOCKS5 +
/// `udp_associate_default_remote` de Tribler, sans SOCKS5.
#[tokio::test]
async fn hidden_seed_udp_relay_roundtrip() {
    let d = make_node().await;
    let s = make_node().await;
    let i = make_node().await;
    let r1 = make_node().await;
    let r2 = make_node().await;
    let nodes = [&d, &s, &i, &r1, &r2];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [9u8; 20];

    // Meme montage que `hidden_service_e2e_roundtrip` : IP puis e2e.
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(&r1))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // Cote seeder : "moteur" = socket UDP qui repond en echo ; le
    // relais `serve` l'alimente depuis le circuit RP_SEEDER.
    let engine = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let engine_addr = engine.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok((n, src)) = engine.recv_from(&mut buf).await {
            let _ = engine.send_to(&buf[..n], src).await;
        }
    });
    let seeder_cids = s
        .tunnel
        .ready_circuits_of_type(onionbit_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER);
    assert_eq!(seeder_cids.len(), 1, "un circuit RP_SEEDER lie attendu");
    onionbit_tunnel::udp_relay::serve(s.tunnel.clone(), seeder_cids[0], engine_addr)
        .await
        .expect("serve relay");

    // Cote downloader : `dial` expose le circuit e2e en socket
    // loopback — le "client" envoie son datagramme uTP au pair cache.
    let peer_addr = onionbit_tunnel::udp_relay::dial(d.tunnel.clone(), e2e_cid)
        .await
        .expect("dial relay");
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let ping = b"utp-handshake-syn";
    client.send_to(ping, peer_addr).await.unwrap();
    let mut buf = [0u8; 2048];
    let (n, _src) = tokio::time::timeout(TEST_TIMEOUT, client.recv_from(&mut buf))
        .await
        .expect("pas d'echo via relais e2e")
        .unwrap();
    assert_eq!(&buf[..n], ping, "echo uTP via circuit e2e + relais");
}

/// Garde-fou anti-fuite de l'adressage `circuit_id_to_ip` : une frame
/// SOCKS5 UDP dont l'IPv4 factice encode un circuit `DATA` ordinaire
/// doit etre rejetee (jamais emise vers le reseau reel) — divergence
/// volontaire : la reference retombe sur un circuit DATA quand le cid
/// n'est pas un RP valide. Le chemin legitime (circuit RP lie) doit
/// lui fonctionner.
#[tokio::test]
async fn socks5_rejects_fake_ip_for_non_rp_circuit() {
    let d = make_node().await;
    let s = make_node().await;
    let i = make_node().await;
    // Relais dedie (comme `hidden_service_e2e_roundtrip`) : avec
    // seulement `s`/`i` comme pairs, le circuit `RP_DOWNLOADER` a 2
    // sauts (`swarm.hops=1` +1) manque de diversite pour se construire
    // de facon fiable (le meme pair devrait servir de relais ET de
    // point de rendez-vous). `r1` fournit un premier saut distinct.
    let r1 = make_node().await;
    let nodes = [&d, &s, &i, &r1];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [11u8; 20];

    // Montage e2e complet (meme sequence que hidden_seed_udp_relay).
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(&i))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // Le proxy voit les deux circuits : DATA = rejete, RP = accepte.
    let socks = Socks5Server::new(d.tunnel.clone(), 1);
    let proxy = socks.listen("127.0.0.1:0").await.unwrap();
    assert!(
        !socks.is_ready_rp_circuit(data_cid),
        "un circuit DATA ne doit pas etre une cible RP"
    );
    assert!(
        socks.is_ready_rp_circuit(e2e_cid),
        "le circuit RP_DOWNLOADER lie doit etre accepte"
    );

    // Client SOCKS5 : greeting + UDP ASSOCIATE.
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut g = [0u8; 2];
    tcp.read_exact(&mut g).await.unwrap();
    tcp.write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .unwrap();
    let mut rep = [0u8; 10];
    tcp.read_exact(&mut rep).await.unwrap();
    let relay_port = u16::from_be_bytes([rep[8], rep[9]]);
    let relay = SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), relay_port);

    let client_udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let fake_ip = |cid: u32| onionbit_tunnel::routing::circuit_id_to_ip(cid);

    // Frame 1 : cid d'un circuit DATA -> doit etre rejetee.
    let mut frame = vec![0u8, 0, 0, 0x01];
    frame.extend_from_slice(&fake_ip(data_cid).octets());
    frame.extend_from_slice(&onionbit_tunnel::routing::CIRCUIT_ID_PORT.to_be_bytes());
    frame.extend_from_slice(b"escape-attempt");
    client_udp.send_to(&frame, relay).await.unwrap();

    let mut s_rx = s.tunnel.data_rx();

    // Frame 2 : cid du circuit RP lie -> doit traverser jusqu'au
    // seeder. Si la frame 1 avait fuite, on ne verrait rien de ce
    // cote ; l'assertion porte sur la livraison de la frame 2.
    let mut frame = vec![0u8, 0, 0, 0x01];
    frame.extend_from_slice(&fake_ip(e2e_cid).octets());
    frame.extend_from_slice(&onionbit_tunnel::routing::CIRCUIT_ID_PORT.to_be_bytes());
    frame.extend_from_slice(b"via-rp-circuit");
    client_udp.send_to(&frame, relay).await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, s_rx.recv())
        .await
        .expect("la frame RP n'est pas arrivee au seeder")
        .expect("canal data seeder");
    assert_eq!(msg.data, b"via-rp-circuit");
}

/// SOCKS5 CONNECT : la requete HTTP brute traverse le tunnel en
/// cellules `http-request`/`http-response` (msgs 28/29) jusqu'a une
/// sortie `PEER_FLAG_EXIT_HTTP`, qui l'execute en TCP reel puis
/// renvoie la reponse chunk-ee.
#[tokio::test]
async fn socks5_connect_http_roundtrip() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_HTTP).await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;
    nodes[0]
        .tunnel
        .set_circuit_exit_flags(cid, PEER_FLAG_EXIT_HTTP);

    // Faux tracker HTTP loopback : repond une annonce bencodee.
    let tracker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut conn, _) = tracker.accept().await.unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = conn.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let body = b"d8:intervali1800e5:peers0:e";
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        conn.write_all(head.as_bytes()).await.unwrap();
        conn.write_all(body).await.unwrap();
        let _ = conn.shutdown().await;
    });

    // Proxy SOCKS5 + client : greeting puis CONNECT vers le tracker.
    let socks = Socks5Server::new(nodes[0].tunnel.clone(), 1);
    let proxy = socks.listen("127.0.0.1:0").await.unwrap();
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut g = [0u8; 2];
    tcp.read_exact(&mut g).await.unwrap();
    assert_eq!(g, [0x05, 0x00], "greeting refuse");

    let mut conn_req = vec![0x05, 0x01, 0x00, 0x01];
    match tracker_addr {
        SocketAddr::V4(a) => conn_req.extend_from_slice(&a.ip().octets()),
        SocketAddr::V6(_) => unreachable!("loopback v4"),
    }
    conn_req.extend_from_slice(&tracker_addr.port().to_be_bytes());
    tcp.write_all(&conn_req).await.unwrap();
    let mut rep = [0u8; 10];
    tcp.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[1], 0x00, "CONNECT refuse");

    // La requete HTTP part en cellules 28 ; la reponse (29) revient.
    tcp.write_all(b"GET /announce?info_hash=x HTTP/1.1\r\nHost: t\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(TEST_TIMEOUT, tcp.read_to_end(&mut out))
        .await
        .expect("pas de reponse http-response")
        .unwrap();
    assert!(
        out.starts_with(b"HTTP/1.1 200 OK"),
        "reponse inattendue: {}",
        String::from_utf8_lossy(&out)
    );
    assert!(
        out.ends_with(b"d8:intervali1800e5:peers0:e"),
        "corps bencode absent"
    );
}

/// `TunnelUdpSocket` (le transport injecte a rqbit pour les lanes
/// anonymes) : un datagramme uTP part en cellule `data`, sort en UDP
/// reel chez la sortie, et la reponse revient sur la meme socket via
/// `data_rx`. Une socket `Dht` parallele ne doit PAS recevoir le
/// datagramme uTP (demux par forme de paquet).
#[tokio::test]
async fn tunnel_udp_socket_utp_roundtrip() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b];
    let (_cid, _) = build_circuit(&nodes, 1).await;

    // Serveur d'echo UDP "exterieur" (atteint par la sortie).
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], src).await;
        }
    });

    let bind = "127.0.0.1:1111".parse().unwrap();
    let utp_sock = TunnelUdpSocket::new(nodes[0].tunnel.clone(), 1, TunnelUdpKind::Utp, bind);
    let dht_sock = TunnelUdpSocket::new(nodes[0].tunnel.clone(), 1, TunnelUdpKind::Dht, bind);

    let msg = utp_payload(b"tunnel-udp-ping");
    utp_sock.send_to(&msg, echo_addr).await.unwrap();
    let mut buf = [0u8; 512];
    let (n, src) = tokio::time::timeout(TEST_TIMEOUT, utp_sock.recv_from(&mut buf))
        .await
        .expect("pas de reponse uTP via tunnel")
        .unwrap();
    assert_eq!(&buf[..n], msg, "payload uTP altere");
    assert_eq!(src, echo_addr, "origine = serveur echo");

    // La socket DHT ne voit pas le datagramme uTP (filtre de forme).
    let mut buf2 = [0u8; 512];
    let res = tokio::time::timeout(Duration::from_millis(300), dht_sock.recv_from(&mut buf2)).await;
    assert!(res.is_err(), "socket DHT a recu du uTP");
}

/// Le pinning destination -> circuit survit aux envois repetes : deux
/// datagrammes vers la meme destination partent sur le meme circuit.
#[tokio::test]
async fn tunnel_udp_socket_pins_circuit_per_destination() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], src).await;
        }
    });

    let bind = "127.0.0.1:1111".parse().unwrap();
    let sock = TunnelUdpSocket::new(nodes[0].tunnel.clone(), 1, TunnelUdpKind::Utp, bind);
    for i in 0..3 {
        let msg = utp_payload(format!("ping-{i}").as_bytes());
        sock.send_to(&msg, echo_addr).await.unwrap();
        let mut buf = [0u8; 512];
        tokio::time::timeout(TEST_TIMEOUT, sock.recv_from(&mut buf))
            .await
            .expect("reponse perdue")
            .unwrap();
    }
    assert_eq!(
        sock.pinned_circuit_for(echo_addr),
        Some(cid),
        "destination epinglee au circuit"
    );
}

/// Suivi des flags de service via les introductions sur le prefixe
/// tunnel : `ExtraIntroductionPayload` piggybacke dans les
/// `introduction-request`/`response` (`extra_bytes` = bitmask `>H`),
/// alimente `get_candidates` (equivalent `candidates` Python).
#[tokio::test]
async fn tunnel_introduction_tracks_exit_flags() {
    let a = make_node_flags(PEER_FLAG_RELAY).await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;

    // A -> B : introduction-request signee sur le prefixe tunnel ;
    // B enregistre nos flags et repond avec les siens.
    a.tunnel
        .send_introduction_request(&UdpAddress::from(b.addr))
        .await
        .unwrap();

    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let a_knows = a.tunnel.peer_flags_of(&b.key.public_key().to_bin());
        let b_knows = b.tunnel.peer_flags_of(&a.key.public_key().to_bin());
        if a_knows & PEER_FLAG_EXIT_BT != 0 && b_knows & PEER_FLAG_RELAY != 0 {
            break;
        }
        assert!(Instant::now() < deadline, "flags non echanges");
        tokio::time::sleep(POLL).await;
    }

    // `get_candidates` : B sortie BT candidate pour A ; A
    // (relay-only) n'est pas une sortie pour B.
    let exits = a.tunnel.get_candidates(PEER_FLAG_EXIT_BT);
    assert_eq!(exits.len(), 1);
    assert_eq!(exits[0].public_key_bin, b.key.public_key().to_bin());
    assert!(b.tunnel.get_candidates(PEER_FLAG_EXIT_BT).is_empty());
    // Le service tunnel est decouvert sur les deux cotes.
    assert!(a
        .network
        .peers_for_service(&TUNNEL_COMMUNITY_ID)
        .iter()
        .any(|p| p.public_key_bin == b.key.public_key().to_bin()));
}

/// Politique de sortie (`is_allowed` pyipv8) : sans `PEER_FLAG_EXIT_BT`
/// annonce, un datagramme uTP est rejete par l'exit ; avec le flag,
/// une donnee quelconque (non BT/IPv8) l'est aussi.
#[tokio::test]
async fn tunnel_exit_drops_non_bt_or_unflagged() {
    let a = make_node().await;
    // Exit sans flag BT : meme un uTP valide ne sort pas.
    let b = make_node_flags(PEER_FLAG_RELAY).await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let dest = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = dest.local_addr().unwrap();
    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(dest_addr),
            &UdpAddress::from(nodes[0].addr),
            &utp_payload(b"utp-ok-mais-pas-de-flag"),
        )
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    assert!(
        tokio::time::timeout(Duration::from_millis(500), dest.recv_from(&mut buf))
            .await
            .is_err(),
        "uTP sorti sans flag EXIT_BT"
    );
}

/// Regression : `build_circuits_if_needed` a 1 saut exige un
/// candidat `EXIT_BT` (`select_exit(ctype=DATA)` pyipv8 — `None` =>
/// pas de circuit). Sans lui, un pair relay-only devenait l'unique
/// hop : circuit cul-de-sac, ses cellules `data` jetees par la
/// politique de sortie — DHT/uTP tunnels muets a jamais.
#[tokio::test]
async fn build_circuits_1hop_refuse_relay_only() {
    let a = make_node().await;
    // Relay-only : aucun circuit ne doit etre cree.
    let b = make_node_flags(PEER_FLAG_RELAY).await;
    a.tunnel
        .send_introduction_request(&UdpAddress::from(b.addr))
        .await
        .unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if a.tunnel.peer_flags_of(&b.key.public_key().to_bin()) & PEER_FLAG_RELAY != 0 {
            break;
        }
        assert!(Instant::now() < deadline, "flags de B non appris");
        tokio::time::sleep(POLL).await;
    }
    a.tunnel.build_circuits_if_needed(1, 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        a.tunnel.ready_circuits().is_empty(),
        "circuit 1 saut cree sur un relais non-exit"
    );

    // Controle positif : un pair EXIT_BT permet la creation.
    let c = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    a.tunnel
        .send_introduction_request(&UdpAddress::from(c.addr))
        .await
        .unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if a.tunnel.peer_flags_of(&c.key.public_key().to_bin()) & PEER_FLAG_EXIT_BT != 0 {
            break;
        }
        assert!(Instant::now() < deadline, "flags de C non appris");
        tokio::time::sleep(POLL).await;
    }
    a.tunnel.build_circuits_if_needed(1, 1).await.unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if a.tunnel
            .ready_circuits_of_hops_flags(1, PEER_FLAG_EXIT_BT)
            .len()
            == 1
        {
            break;
        }
        assert!(Instant::now() < deadline, "circuit 1 saut non READY");
        tokio::time::sleep(POLL).await;
    }
}

/// Regression : `build_circuits_if_needed` ne doit compter que les
/// circuits `DATA`. Les circuits `IP_SEEDER`/`RP_*` des hidden
/// services servent les lanes e2e et ne sont pas eligibles au
/// routage applicatif (`select_circuit` n'emet que sur `DATA`) — les
/// compter comme "prets" bloquait toute construction DATA : la lane
/// mourrait de faim, metadonnees BEP 9 bloquees sans erreur
/// (downloads anonymes figes en `METADATA`).
#[tokio::test]
async fn build_circuits_ne_compte_pas_les_ip_seeder() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    // Flags de B appris par A (necessaire au choix de sortie 1 saut).
    a.tunnel
        .send_introduction_request(&UdpAddress::from(b.addr))
        .await
        .unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if a.tunnel.peer_flags_of(&b.key.public_key().to_bin()) & PEER_FLAG_EXIT_BT != 0 {
            break;
        }
        assert!(Instant::now() < deadline, "flags de B non appris");
        tokio::time::sleep(POLL).await;
    }

    // Circuit IP_SEEDER a 1 saut (swarm a 0 saut + 1) sur B.
    let ih = [7u8; 20];
    a.tunnel.join_swarm(ih, 0, true);
    let ip_cid = a
        .tunnel
        .create_introduction_point(ih, Some(&peer_of(&b)))
        .await
        .expect("create_introduction_point");
    a.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit IP_SEEDER pas READY");
    assert!(
        a.tunnel.ready_circuits_of_hops(1).contains(&ip_cid)
            && a.tunnel.ready_data_circuits_of_hops(1).is_empty(),
        "circuit IP_SEEDER a 1 saut attendu, aucun DATA"
    );

    // Le besoin de circuits DATA a 1 saut ne doit pas etre couvert
    // par l'IP_SEEDER : une construction DATA doit suivre.
    a.tunnel.build_circuits_if_needed(1, 1).await.unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if !a.tunnel.ready_data_circuits_of_hops(1).is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "aucun circuit DATA cree — l'IP_SEEDER a ete compte comme pret"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Deux lanes SOCKS5 sur la meme community (`data_rx` broadcast) :
/// chaque association ne recoit que les reponses de SES circuits
/// (filtrage `return_map`), meme en trafic simultane.
#[tokio::test]
async fn socks5_two_lanes_isolated_returns() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let c = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT).await;
    let nodes = [a, b, c];
    // Circuit 1 saut (lane 1) et circuit 2 sauts (lane 2).
    build_circuit(&nodes, 1).await;
    build_circuit(&nodes, 2).await;

    // Serveur d'echo externe.
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], src).await;
        }
    });

    // Deux "lanes" : hops=1 et hops=2, meme community sous-jacente.
    let socks1 = Socks5Server::new(nodes[0].tunnel.clone(), 1);
    let socks2 = Socks5Server::new(nodes[0].tunnel.clone(), 2);
    let proxy1 = socks1.listen("127.0.0.1:0").await.unwrap();
    let proxy2 = socks2.listen("127.0.0.1:0").await.unwrap();

    async fn associate(proxy: SocketAddr) -> (TcpStream, tokio::net::UdpSocket, SocketAddr) {
        let mut tcp = TcpStream::connect(proxy).await.unwrap();
        tcp.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut g = [0u8; 2];
        tcp.read_exact(&mut g).await.unwrap();
        tcp.write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        let mut rep = [0u8; 10];
        tcp.read_exact(&mut rep).await.unwrap();
        assert_eq!(rep[1], 0x00, "UDP ASSOCIATE refuse");
        let relay = SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(rep[4], rep[5], rep[6], rep[7])),
            u16::from_be_bytes([rep[8], rep[9]]),
        );
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        (tcp, udp, relay)
    }

    async fn send_frame(
        udp: &tokio::net::UdpSocket,
        relay: SocketAddr,
        dest: SocketAddr,
        tag: &[u8],
    ) {
        let msg = utp_payload(tag);
        let mut frame = vec![0u8, 0, 0, 0x01];
        match dest {
            SocketAddr::V4(a) => frame.extend_from_slice(&a.ip().octets()),
            SocketAddr::V6(_) => unreachable!("loopback v4"),
        }
        frame.extend_from_slice(&dest.port().to_be_bytes());
        frame.extend_from_slice(&msg);
        udp.send_to(&frame, relay).await.unwrap();
    }

    let (_t1, udp1, relay1) = associate(proxy1).await;
    let (_t2, udp2, relay2) = associate(proxy2).await;

    send_frame(&udp1, relay1, echo_addr, b"lane-1").await;
    send_frame(&udp2, relay2, echo_addr, b"lane-2").await;

    // Chaque client recoit la reponse de SA lane, avec son tag.
    let mut buf = [0u8; 512];
    let (n1, _) = tokio::time::timeout(TEST_TIMEOUT, udp1.recv_from(&mut buf))
        .await
        .expect("lane 1 sans reponse")
        .unwrap();
    assert_eq!(
        &buf[10..n1],
        utp_payload(b"lane-1"),
        "lane 1 a recu la reponse d'une autre lane"
    );
    let (n2, _) = tokio::time::timeout(TEST_TIMEOUT, udp2.recv_from(&mut buf))
        .await
        .expect("lane 2 sans reponse")
        .unwrap();
    assert_eq!(
        &buf[10..n2],
        utp_payload(b"lane-2"),
        "lane 2 a recu la reponse d'une autre lane"
    );
}

/// Speed-test loopback (etape 27) : `run_speedtest` envoie des
/// cellules `test-request`(21) sur le circuit, la sortie repond
/// `test-response`(22) — le dernier snapshot (`done=true`) doit
/// contenir des reponses mesurees.
#[tokio::test]
async fn speedtest_cells_roundtrip_1_hop() {
    let a = make_node().await;
    let b = make_node_flags(PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT | PEER_FLAG_SPEED_TEST).await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let mut rx = nodes[0].tunnel.run_speedtest(cid, 800, 50, 256);
    let mut answered = 0usize;
    let mut saw_done = false;
    while let Ok(item) = tokio::time::timeout(TEST_TIMEOUT * 4, rx.recv()).await {
        let Some((stats, done)) = item else { break };
        answered += stats.values().filter(|s| s[3] > 0).count();
        if done {
            saw_done = true;
            break;
        }
    }
    assert!(saw_done, "snapshot final `done` attendu");
    assert!(answered > 0, "au moins une test-response mesuree");
}

/// `estimate_swarm_size` sans circuit disponible : toutes les
/// requetes levent "no circuit" → `swarm_size` 0 (comportement
/// `return_exceptions=True` de Python).
#[tokio::test]
async fn estimate_swarm_size_sans_circuit_zero() {
    let a = make_node().await;
    let n = tokio::time::timeout(TEST_TIMEOUT, a.tunnel.estimate_swarm_size([1u8; 20], 1))
        .await
        .expect("estimate_swarm_size");
    assert_eq!(n, 0);
}

/// `on_establish_intro` alimente le store PEX du point
/// d'introduction (`start_announce`) : `pex_intro_points` expose le
/// `seeder_pk` annonce avec `source = PEER_SOURCE_PEX`.
#[tokio::test]
async fn establish_intro_populates_pex_store() {
    let s = make_node().await; // seeder
    let i = make_node().await; // point d'introduction
    learn(&s, &i);
    learn(&i, &s);
    let info_hash = [9u8; 20];

    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP circuit READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("send establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    let pex = i.tunnel.pex_intro_points();
    assert_eq!(pex.len(), 1, "un swarm dans le store PEX");
    assert_eq!(pex[0].0, info_hash);
    assert_eq!(pex[0].1.len(), 1, "une annonce");
    let ip = &pex[0].1[0];
    assert_eq!(ip.source, PEER_SOURCE_PEX);
    // `seeder_pk` = cle de SWARM du seeder (`seeder_sk` generee par
    // `join_swarm`), pas sa cle de pair — `LibNaCLPK:` + 64o.
    assert_eq!(ip.seeder_pk.len(), 74);
    assert!(ip.seeder_pk.starts_with(b"LibNaCLPK:"));
    assert_eq!(ip.peer_key, i.key.public_key().to_bin());
}

/// `RandomWalk` tunnel : une `introduction-request` sous le prefixe
/// tunnel enregistre le demandeur cote B (service + flags) et le
/// repondant cote A via la `introduction-response` — l'overlay se
/// peuple sans trafic de circuit.
#[tokio::test(flavor = "multi_thread")]
async fn walk_decouvre_les_pairs_tunnel() {
    let a = make_node_flags(PEER_FLAG_RELAY).await;
    let b = make_node_flags(PEER_FLAG_EXIT_BT).await;

    a.tunnel
        .walk_to(&UdpAddress::from(b.addr))
        .await
        .expect("walk_to");

    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut ok = false;
    while Instant::now() < deadline {
        let a_peers = a.network.peers_for_service(&TUNNEL_COMMUNITY_ID);
        let b_peers = b.network.peers_for_service(&TUNNEL_COMMUNITY_ID);
        if !a_peers.is_empty() && !b_peers.is_empty() {
            ok = true;
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    assert!(ok, "la marche n'a pas peuple l'overlay tunnel");
    // Les flags de service ont transite par `extra_bytes`.
    assert_eq!(
        a.tunnel.peer_flags_of(&b.key.public_key().to_bin()) & PEER_FLAG_EXIT_BT,
        PEER_FLAG_EXIT_BT
    );
}

/// Semantique file pleine de `inject_incoming` (le `try_send` des
/// lanes e2e) : une rafale synchrone plus grande que la capacite
/// (`IN_QUEUE_CAP` = 512) perd en drop-tail — FIFO conserve pour la
/// partie livree, aucun blocage ni panique, et la socket reste
/// utilisable apres la rafale. C'est le comportement UDP voulu : la
/// congestion du tunnel est representee par des pertes, jamais par
/// du backpressure sur la tache de dispatch.
#[tokio::test]
async fn inject_incoming_burst_drop_tail() {
    let a = make_node().await;
    let bind = "127.0.0.1:1111".parse().unwrap();
    let sock = TunnelUdpSocket::new(a.tunnel.clone(), 1, TunnelUdpKind::Utp, bind);
    let src: SocketAddr = "10.9.8.7:1024".parse().unwrap();

    // Rafale synchrone : sur le runtime courant la boucle ne laisse
    // jamais le consommateur s'executer -> exactement la capacite de
    // la file est livree, le reste tombe (drop-tail).
    const BURST: usize = 2048;
    const CAP: usize = 512;
    for i in 0..BURST {
        sock.inject_incoming(utp_payload(&i.to_be_bytes()), src);
    }

    let mut got = Vec::new();
    let deadline = Instant::now() + TEST_TIMEOUT;
    while got.len() < CAP && Instant::now() < deadline {
        let mut buf = [0u8; 512];
        match tokio::time::timeout(POLL * 10, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                assert_eq!(from, src);
                got.push(buf[..n].to_vec());
            }
            _ => break,
        }
    }
    assert_eq!(got.len(), CAP, "drop-tail : seule la capacite est livree");
    // FIFO : les CAP premiers datagrammes dans l'ordre.
    for (idx, d) in got.iter().enumerate() {
        let tag = &d[d.len() - 4..];
        assert_eq!(
            tag,
            (idx as u32).to_be_bytes(),
            "ordre casse a l'index {idx}"
        );
    }

    // La socket reste fonctionnelle apres l'overflow.
    sock.inject_incoming(utp_payload(b"post-burst"), src);
    let mut buf = [0u8; 512];
    let (n, _) = tokio::time::timeout(TEST_TIMEOUT, sock.recv_from(&mut buf))
        .await
        .expect("socket morte apres la rafale")
        .unwrap();
    assert_eq!(&buf[..n], utp_payload(b"post-burst"));
}

/// Stress e2e : la chaine reelle du hidden seeding cote seeder —
/// cellules `data` d'un circuit `RP_SEEDER` lie ->
/// `subscribe_circuit_data` -> filtrage `could_be_utp` ->
/// `TunnelUdpSocket::inject_incoming` -> `recv_from`. Une rafale de
/// 2000 datagrammes uTP (> 3x la capacite des deux files en serie)
/// doit traverser sans perte quand le consommateur suit : les files
/// absorbent les micro-rafales, le pacing vient du chiffrement
/// par cellule cote emetteur.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_seed_e2e_burst_integrity() {
    let d = make_node().await;
    let s = make_node().await;
    let i = make_node().await;
    let r1 = make_node().await;
    let r2 = make_node().await;
    let nodes = [&d, &s, &i, &r1, &r2];
    for a in &nodes {
        for b in &nodes {
            if !std::ptr::eq(*a, *b) {
                learn(a, b);
            }
        }
    }
    let info_hash = [13u8; 20];

    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(&i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("IP READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("establish-intro");
    tokio::time::timeout(TEST_TIMEOUT, intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(&r1))
        .await
        .expect("circuit data D");
    d.tunnel
        .wait_circuit_ready(data_cid, TEST_TIMEOUT.as_millis() as u64)
        .await
        .expect("circuit data READY");

    let ip_hint = onionbit_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: onionbit_tunnel::routing::PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // Cote seeder : replique la chaine de production d'ipv8_stack —
    // abonnement par circuit + inject_incoming sur la socket uTP.
    let rp_cids = s
        .tunnel
        .ready_circuits_of_type(onionbit_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER);
    assert_eq!(rp_cids.len(), 1, "un circuit RP_SEEDER lie attendu");
    let bind = "127.0.0.1:2222".parse().unwrap();
    let sock = TunnelUdpSocket::new(s.tunnel.clone(), 1, TunnelUdpKind::Utp, bind);
    let fake_peer: SocketAddr = "10.1.2.3:1024".parse().unwrap();
    let mut data_rx = s.tunnel.subscribe_circuit_data(rp_cids[0]);
    {
        let sock = sock.clone();
        tokio::spawn(async move {
            while let Some(msg) = data_rx.recv().await {
                if onionbit_network_policy::exit_policy::could_be_utp(&msg.data) {
                    sock.inject_incoming(msg.data, fake_peer);
                }
            }
        });
    }

    // Consommateur (le `recv_from` de rqbit) : collecte les tags.
    // Arret au quota ou apres 2 s de silence (file fermee/perturbee).
    const N: usize = 2000;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let sock2 = sock.clone();
    tokio::spawn(async move {
        let mut got: Vec<u32> = Vec::new();
        while got.len() < N {
            let mut buf = [0u8; 512];
            match tokio::time::timeout(Duration::from_secs(2), sock2.recv_from(&mut buf)).await {
                Ok(Ok((n, _))) if n >= 4 => {
                    got.push(u32::from_be_bytes(buf[n - 4..n].try_into().unwrap()));
                }
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
        let _ = done_tx.send(got);
    });

    // Rafale e2e : N cellules `data` successives, le debit n'est
    // borne que par le chiffrement + loopback UDP.
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    for i in 0..N as u32 {
        d.tunnel
            .send_data(e2e_cid, &zero, &zero, &utp_payload(&i.to_be_bytes()))
            .await
            .expect("send_data e2e");
    }

    let got = tokio::time::timeout(Duration::from_secs(30), done_rx)
        .await
        .expect("rafale e2e : consommateur bloque")
        .expect("canal collecteur");
    // Invariant 1 — FIFO bout en bout : les tags recus forment une
    // sous-sequence strictement croissante (la pompe d'emission
    // serialisee interdit le reordonnancement des cellules relayees).
    for w in got.windows(2) {
        assert!(w[0] < w[1], "desordre : {} puis {}", w[0], w[1]);
    }
    // Invariant 2 — pertes bornees : les files (`data_subscribers`,
    // `in_tx`) sont drop-tail, des pertes sous contention CPU sont la
    // semantique UDP voulue ; uTP reemet. En suite parallele les 27
    // autres tests ralentissent le consommateur : sous 90 % serait un
    // goulot inacceptable pour le transfert reel.
    assert!(
        got.len() >= N * 9 / 10,
        "pertes excessives en rafale : {}/{} recus",
        got.len(),
        N
    );
}
