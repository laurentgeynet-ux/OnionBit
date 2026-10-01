//! Telechargement BitTorrent reel a travers un circuit e2e lie
//! (hidden seeding) : deux moteurs rqbit uTP relies par les relais
//! UDP de `onionbit-tunnel`. Tout est en loopback — aucun trafic
//! externe.
//!
//! Montage (jalon de l'etape 12) :
//! - le seeder rejoint le swarm cache et cree un point d'introduction ;
//! - le downloader decouvre l'IP via `peers-request`, etablit un
//!   circuit e2e (`create-e2e` -> rendezvous -> `link-e2e` ->
//!   `linked-e2e`) ;
//! - `udp_relay::serve` alimente la socket uTP du seeder depuis le
//!   circuit RP_SEEDER ; `udp_relay::dial` expose le circuit
//!   RP_DOWNLOADER comme "adresse du pair" pour rqbit ;
//! - rqbit downloader ajoute le torrent avec `initial_peers` =
//!   l'adresse du relais — les datagrammes uTP/BT traversent le
//!   tunnel chiffre dans les deux sens.

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
use onionbit_tunnel::routing::{
    IntroductionPoint, CIRCUIT_TYPE_RP_SEEDER, PEER_FLAG_RELAY, PEER_SOURCE_UNKNOWN,
};
use onionbit_tunnel::udp_relay;
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

/// Delai max de telechargement anonyme (le tunnel + uTP ajoutent de
/// la latence au transfert local).
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

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

fn learn(node: &TunnelNode, other: &TunnelNode) {
    let p = peer_of(other);
    node.network.add_verified(p.clone());
    node.network
        .discover_service(&p.public_key_bin, TUNNEL_COMMUNITY_ID);
}

/// Monte le lien e2e complet seeder <-> downloader pour `info_hash`
/// et retourne `(cid RP_DOWNLOADER, cid RP_SEEDER)`.
async fn link_hidden_circuit(
    d: &TunnelNode,
    s: &TunnelNode,
    i: &TunnelNode,
    info_hash: [u8; 20],
) -> (u32, u32) {
    // Seeder : swarm + point d'introduction vers `i`.
    s.tunnel.join_swarm(info_hash, 0, true);
    let ip_cid = s
        .tunnel
        .create_introduction_point(info_hash, Some(&peer_of(i)))
        .await
        .expect("circuit IP_SEEDER");
    s.tunnel
        .wait_circuit_ready(ip_cid, 5000)
        .await
        .expect("IP READY");
    let intro_rx = s
        .tunnel
        .send_establish_intro(ip_cid, info_hash)
        .await
        .expect("establish-intro");
    tokio::time::timeout(Duration::from_secs(5), intro_rx)
        .await
        .expect("timeout intro-established")
        .expect("intro-established");

    // Downloader : circuit de donnees puis peers-request vers l'IP.
    d.tunnel.join_swarm(info_hash, 1, false);
    let data_cid = d
        .tunnel
        .create_circuit(1, &peer_of(i))
        .await
        .expect("circuit data");
    d.tunnel
        .wait_circuit_ready(data_cid, 5000)
        .await
        .expect("data READY");
    let ip_hint = IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: Vec::new(),
        source: PEER_SOURCE_UNKNOWN,
        last_seen_secs: 0,
    };
    let ips = d
        .tunnel
        .send_peers_request(info_hash, Some(&ip_hint), 1)
        .await
        .expect("peers-response");
    assert_eq!(ips.len(), 1);

    // e2e complet (avec repli : le handshake UDP n'a pas de
    // retransmission protocolaire, une cellule loopback peut se
    // perdre sous charge parallele).
    let mut e2e_rx = d.tunnel.e2e_ready();
    let mut d_cid = None;
    for _ in 0..3 {
        d.tunnel
            .create_e2e(info_hash, &ips[0])
            .await
            .expect("create_e2e");
        if let Ok(Ok((cid, _))) = tokio::time::timeout(Duration::from_secs(20), e2e_rx.recv()).await
        {
            d_cid = Some(cid);
            break;
        }
    }
    let d_cid = d_cid.expect("e2e_ready jamais atteint");
    let seeder_cids = s.tunnel.ready_circuits_of_type(CIRCUIT_TYPE_RP_SEEDER);
    assert_eq!(seeder_cids.len(), 1, "circuit RP_SEEDER lie attendu");
    (d_cid, seeder_cids[0])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_download_via_hidden_service() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=debug")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    // 1. Torrent reel : un fichier dans le dossier du seeder.
    let seed_dir = tempfile::tempdir().unwrap();
    let payload_content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(seed_dir.path().join("fichier.bin"), &payload_content).unwrap();
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
    let info_hash: [u8; 20] = torrent.info_hash().0;

    // 2. Moteur du seeder (uTP only, pas de DHT/trackers).
    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.utp_only = true;
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seeder_listen = {
        // `listen_addr` annonce l'adresse wildcard de la socket —
        // la ramener a loopback pour que le relais puisse y envoyer.
        let a = seeder.listen_addr().expect("ecoute uTP seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };

    // 3. Noeuds tunnel : downloader, seeder, point d'introduction.
    let d = make_node().await;
    let s = make_node().await;
    let i = make_node().await;
    for a in [&d, &s, &i] {
        for b in [&d, &s, &i] {
            if !std::ptr::eq(a, b) {
                learn(a, b);
            }
        }
    }
    let (d_cid, s_cid) = link_hidden_circuit(&d, &s, &i, info_hash).await;

    // 4. Relais : le seeder sert sa socket uTP ; le downloader expose
    //    le circuit comme adresse de pair.
    udp_relay::serve(s.tunnel.clone(), s_cid, seeder_listen)
        .await
        .expect("serve relay");
    let peer_addr = udp_relay::dial(d.tunnel.clone(), d_cid)
        .await
        .expect("dial relay");

    // 5. Moteur du downloader : pair = adresse du relais.
    let dl_dir = tempfile::tempdir().unwrap();
    let mut dl_cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
    dl_cfg.utp_only = true;
    dl_cfg.listen_port = Some(0);
    let downloader = BtEngine::start(dl_cfg).await.expect("downloader engine");
    let dl = downloader
        .add_with_options(
            librqbit::AddTorrent::from_bytes(torrent_bytes),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                initial_peers: Some(vec![peer_addr]),
                ..Default::default()
            }),
        )
        .await
        .expect("add download");

    // 6. Le telechargement complet doit passer par le tunnel.
    tokio::time::timeout(TEST_TIMEOUT, dl.wait_completed())
        .await
        .expect("telechargement anonyme en timeout")
        .expect("wait_completed");

    let got = std::fs::read(dl_dir.path().join("fichier.bin")).unwrap();
    assert_eq!(got, payload_content, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
}
