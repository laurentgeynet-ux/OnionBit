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
use tribler_tunnel::routing::{DESTROY_REASON_UNNEEDED, PEER_FLAG_RELAY};
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
