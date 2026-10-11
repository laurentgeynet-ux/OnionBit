// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Repro isolée du backend `bitdaemon` (ONIONBIT-1) : même config que
//! le daemon en mode direct, ajout d'un `.torrent` réel, sondage du
//! progrès. `#[ignore]` — swarm réel, lancer à la main :
//! `cargo test -p onionbit-bittorrent --test bd_backend_smoke -- --ignored --nocapture`

use onionbit_bittorrent::{BtEngine, EngineBackend, EngineConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bd_backend_telecharge_un_torrent_reel() {
    tracing_subscriber::fmt()
        .with_env_filter("info,bitdaemon=debug,onionbit=debug")
        .try_init()
        .ok();
    let dl_dir = std::env::var("BD_TEST_DL").unwrap_or_else(|_| "C:/ob-bench/dl-test".into());
    let torrent =
        std::env::var("BD_TEST_TORRENT").unwrap_or_else(|_| "C:/ob-bench/debian.torrent".into());
    std::fs::create_dir_all(&dl_dir).unwrap();

    // Config miroir de `engine_from_config` (daemon, mode direct).
    let cfg = EngineConfig {
        backend: EngineBackend::Bitdaemon,
        output_dir: dl_dir.clone().into(),
        listen_port: Some(45020),
        enable_dht: true,
        disable_lsd: false,
        enable_utp: true,
        enable_upnp: true,
        enable_natpmp: true,
        allow_mmap: false,
        clear_orphaned_parts: false,
        fastresume_sampled_check: true,
        persistence_dir: Some(std::path::PathBuf::from(format!("{dl_dir}/persist"))),
        ..Default::default()
    };
    let engine = BtEngine::start(cfg).await.unwrap();

    // Chemin d'ajout du daemon : `add_torrent_bytes_opts` + options de
    // ligne (destination incluant le nom, trackers effectifs…).
    let bytes = std::fs::read(&torrent).unwrap();
    let opts = onionbit_bittorrent::add_options::AddDownloadOptions {
        paused: false,
        output_folder: Some(dl_dir.clone().into()),
        output_includes_name: true,
        ..Default::default()
    };
    let dl = engine.add_torrent_bytes_opts(bytes, &opts).await.unwrap();
    eprintln!("added {}", dl.info_hash_hex());

    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let s = dl.stats();
        eprintln!(
            "{:.1}% | down {} B/s | peers {} | {:?} | err {:?}",
            s.progress() * 100.0,
            s.download_speed,
            s.peers_live,
            s.state,
            s.error
        );
        if s.finished {
            break;
        }
    }
    engine.stop().await;
}
