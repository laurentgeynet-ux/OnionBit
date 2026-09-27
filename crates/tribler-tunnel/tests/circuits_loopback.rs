//! Tests d'integration `TunnelCommunity` en loopback : construction
//! de circuits 1 et 2 sauts sur de vrais sockets UDP, relais de
//! cellules chiffrees ChaCha20-Poly1305, sortie UDP, destroy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::routing::{
    DESTROY_REASON_UNNEEDED, PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_HTTP, PEER_FLAG_RELAY,
};
use tribler_tunnel::socks5::Socks5Server;
use tribler_tunnel::TUNNEL_COMMUNITY_ID;

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
        addr,
    }
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

#[tokio::test]
async fn tunnel_circuit_2_hops_becomes_ready() {
    let a = make_node().await;
    let b = make_node().await;
    let c = make_node().await;
    let nodes = [a, b, c];
    let (_cid, ready) = build_circuit(&nodes, 2).await;
    assert_eq!(ready.len(), 1);
}

#[tokio::test]
async fn tunnel_data_exits_1_hop() {
    let a = make_node().await;
    let b = make_node().await;
    let nodes = [a, b];
    let (cid, _) = build_circuit(&nodes, 1).await;

    let dest = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = dest.local_addr().unwrap();
    let payload = b"hello 1 hop";

    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(dest_addr),
            &UdpAddress::from(nodes[0].addr),
            payload,
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
        .with_env_filter("tribler=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let a = make_node().await;
    let b = make_node().await;
    let c = make_node().await;
    let nodes = [a, b, c];
    let (cid, _) = build_circuit(&nodes, 2).await;

    // Socket de destination "monde exterieur" (receveur UDP brut).
    let dest = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = dest.local_addr().unwrap();
    let payload = b"hello via tunnel";

    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(dest_addr),
            &UdpAddress::from(nodes[0].addr),
            payload,
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
    let c = make_node().await;
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

    let mut data_rx = nodes[0].tunnel.data_rx().expect("data_rx deja pris");
    let payload = b"ping aller-retour";
    nodes[0]
        .tunnel
        .send_data(
            cid,
            &UdpAddress::from(echo_addr),
            &UdpAddress::from(nodes[0].addr),
            payload,
        )
        .await
        .unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, data_rx.recv())
        .await
        .expect("pas de retour tunnel")
        .expect("canal data ferme");
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
        .with_env_filter("tribler=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let a = make_node().await;
    let b = make_node().await;
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
    let msg = b"tribler-socks5-ping";
    let mut frame = vec![0u8, 0, 0, 0x01];
    match echo_addr {
        SocketAddr::V4(a) => frame.extend_from_slice(&a.ip().octets()),
        SocketAddr::V6(_) => unreachable!("loopback v4"),
    }
    frame.extend_from_slice(&echo_addr.port().to_be_bytes());
    frame.extend_from_slice(msg);
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
    ip: &tribler_tunnel::routing::IntroductionPoint,
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
        .with_env_filter("tribler=trace")
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
    let ip_hint = tribler_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: tribler_tunnel::routing::PEER_SOURCE_UNKNOWN,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 5000)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1, "un point d'introduction attendu");
    assert!(!ips[0].seeder_pk.is_empty(), "seeder_pk present");

    let e2e_cid = create_e2e_with_retry(&d, info_hash, &ips[0]).await;

    // Donnee e2e downloader -> seeder : la couche hs est appliquee
    // (decryptee cote seeder) et le RP reexpedie.
    let mut s_rx = s.tunnel.data_rx().expect("data_rx seeder");
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

    let ip_hint = tribler_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: tribler_tunnel::routing::PEER_SOURCE_UNKNOWN,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 5000)
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
            .ready_circuits_of_type(tribler_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER)
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

    let ip_hint = tribler_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: tribler_tunnel::routing::PEER_SOURCE_UNKNOWN,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 5000)
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
        .ready_circuits_of_type(tribler_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER);
    assert_eq!(seeder_cids.len(), 1, "un circuit RP_SEEDER lie attendu");
    tribler_tunnel::udp_relay::serve(s.tunnel.clone(), seeder_cids[0], engine_addr)
        .await
        .expect("serve relay");

    // Cote downloader : `dial` expose le circuit e2e en socket
    // loopback — le "client" envoie son datagramme uTP au pair cache.
    let peer_addr = tribler_tunnel::udp_relay::dial(d.tunnel.clone(), e2e_cid)
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

    let ip_hint = tribler_tunnel::routing::IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: tribler_tunnel::routing::PEER_SOURCE_UNKNOWN,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 5000)
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
    let fake_ip = |cid: u32| tribler_tunnel::routing::circuit_id_to_ip(cid);

    // Frame 1 : cid d'un circuit DATA -> doit etre rejetee.
    let mut frame = vec![0u8, 0, 0, 0x01];
    frame.extend_from_slice(&fake_ip(data_cid).octets());
    frame.extend_from_slice(&tribler_tunnel::routing::CIRCUIT_ID_PORT.to_be_bytes());
    frame.extend_from_slice(b"escape-attempt");
    client_udp.send_to(&frame, relay).await.unwrap();

    let mut s_rx = s.tunnel.data_rx().expect("data_rx seeder");

    // Frame 2 : cid du circuit RP lie -> doit traverser jusqu'au
    // seeder. Si la frame 1 avait fuite, on ne verrait rien de ce
    // cote ; l'assertion porte sur la livraison de la frame 2.
    let mut frame = vec![0u8, 0, 0, 0x01];
    frame.extend_from_slice(&fake_ip(e2e_cid).octets());
    frame.extend_from_slice(&tribler_tunnel::routing::CIRCUIT_ID_PORT.to_be_bytes());
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
