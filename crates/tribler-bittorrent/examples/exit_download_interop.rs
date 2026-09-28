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
//! Usage :
//!   exit_download_interop --keyfile F --hops 2 [--payload N] [--log L]
//! `--keyfile` contient `<pubkey_hex> <port>` ecrit par
//! `py_tunnel_node.py`. `--hops 1` supprime le relais Rust (circuit
//! direct vers la sortie Python).

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
use tribler_tunnel::udp_relay;

/// Delai max de telechargement a travers le tunnel.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("tribler=info")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let mut keyfile = None;
    let mut hops = 2usize;
    let mut payload_len = DEFAULT_PAYLOAD;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--keyfile" => keyfile = args.next(),
            "--hops" => hops = args.next().unwrap().parse().unwrap(),
            "--payload" => payload_len = args.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }
    let keyfile = keyfile.expect("--keyfile requis");

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

    // 1. Torrent reel dans le dossier du seeder.
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

    // 2. Seeder rqbit (uTP uniquement, pas de DHT/trackers).
    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.utp_only = true;
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seeder_listen = {
        let a = seeder.listen_addr().expect("ecoute uTP seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };

    // 3. Noeuds tunnel : downloader + relais Rust (2 sauts : relais
    //    Rust -> sortie Python ; `--hops 1` = sortie directe).
    let d = make_node().await;
    let relay = if hops >= 2 {
        Some(make_node().await)
    } else {
        None
    };
    d.network.add_verified(py_peer.clone());
    let cid = match &relay {
        Some(r) => {
            let rp = peer_of(r);
            d.network.add_verified(rp.clone());
            d.tunnel
                .create_circuit_typed(
                    hops,
                    &rp,
                    tribler_tunnel::routing::CIRCUIT_TYPE_DATA,
                    Some(py_pk),
                    None,
                )
                .await
                .expect("create_circuit 2 sauts")
        }
        None => d
            .tunnel
            .create_circuit(1, &py_peer)
            .await
            .expect("create_circuit 1 saut"),
    };
    if d.tunnel
        .wait_circuit_ready(cid, READY_TIMEOUT_MS)
        .await
        .is_err()
    {
        eprintln!("ECHEC : circuit {cid} jamais READY");
        std::process::exit(1);
    }
    eprintln!("circuit {cid} READY ({hops} saut(s), sortie = pyipv8)");

    // 4. Pont : la socket `dial` est l'adresse du "pair" pour rqbit ;
    //    les datagrammes partent en cellules `data` destinees a la
    //    socket uTP du seeder a travers la sortie Python.
    let peer_addr = udp_relay::dial_to(d.tunnel.clone(), cid, UdpAddress::from(seeder_listen))
        .await
        .expect("dial_to relay");

    // 5. Downloader rqbit : pair = adresse du pont.
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

    tokio::time::timeout(DOWNLOAD_TIMEOUT, dl.wait_completed())
        .await
        .expect("telechargement via sortie pyipv8 en timeout")
        .expect("wait_completed");

    let got = std::fs::read(dl_dir.path().join("fichier.bin")).unwrap();
    assert_eq!(got, payload, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
    eprintln!("INTEROP EXIT DOWNLOAD OK ({} octets)", payload.len());
}
