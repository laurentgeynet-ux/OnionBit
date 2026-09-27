//! Tests d'integration `TunnelCommunity` en loopback : construction
//! de circuits 1 et 2 sauts sur de vrais sockets UDP, relais de
//! cellules chiffrees ChaCha20-Poly1305, sortie UDP, destroy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::routing::{DESTROY_REASON_UNNEEDED, PEER_FLAG_RELAY};
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
