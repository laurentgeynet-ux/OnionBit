//! Telechargement BitTorrent reel **non anonyme** en loopback : un
//! seeder et un downloader rqbit relies directement par uTP. Aucun
//! trafic externe — le pair initial du downloader est l'adresse
//! loopback d'ecoute du seeder.

use std::net::SocketAddr;
use std::time::Duration;

use tribler_bittorrent::config::EngineConfig;
use tribler_bittorrent::engine::BtEngine;

/// Delai max du transfert local (uTP sur loopback).
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_download_complet() {
    // 1. Torrent reel : un fichier dans le dossier du seeder.
    let seed_dir = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..120_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(seed_dir.path().join("payload.bin"), &payload).unwrap();
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

    // 2. Seeder : moteur offline + ecoute uTP.
    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.utp_only = true;
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    // `listen_addr` annonce l'adresse wildcard — ramener a loopback.
    let seed_addr = {
        let a = seeder.listen_addr().expect("ecoute uTP seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };

    // 3. Downloader : le seeder est le pair initial.
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
                initial_peers: Some(vec![seed_addr]),
                ..Default::default()
            }),
        )
        .await
        .expect("add download");

    // 4. Transfert complet + integrite du contenu.
    tokio::time::timeout(TEST_TIMEOUT, dl.wait_completed())
        .await
        .expect("telechargement en timeout")
        .expect("wait_completed");
    let got = std::fs::read(dl_dir.path().join("payload.bin")).unwrap();
    assert_eq!(got, payload, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
}
