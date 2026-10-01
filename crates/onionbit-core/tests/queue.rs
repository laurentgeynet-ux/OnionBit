// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Gestionnaire de file libtorrent (`active_downloads`/`active_seeds`/
//! `active_limit`) : les telechargements `auto_managed` au-dela des
//! bornes sont pauses par `CoreSession` et repris quand des slots se
//! liberent — sans toucher a `paused`/`user_stopped` persistes
//! (distinction `queued` Python).

use onionbit_bittorrent::DownloadState;
use onionbit_core::{CoreConfig, CoreSession, Notifier};

/// Attend `f` jusqu'a `deadline` (ticks courts : la boucle de
/// progression tourne a `progress_interval_ms`).
async fn wait_until(f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if f() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("condition non atteinte dans le delai");
}

/// `active_downloads = 1` : sur deux telechargements `auto_managed`
/// actifs, le dernier de la file est mis en pause ; a la suppression
/// de l'actif, le mis en file reprend.
#[tokio::test]
async fn queue_manager_pause_et_reprend_les_auto_managed() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.queue.active_downloads = 1;
    cfg.queue.active_seeds = -1;
    cfg.queue.active_limit = -1;
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start");

    let a = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("a.bin", 42),
            false,
        )
        .await
        .unwrap();
    let b = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("b.bin", 42),
            false,
        )
        .await
        .unwrap();
    let (ha, hb) = (a.info_hash_hex(), b.info_hash_hex());
    session.set_auto_managed(&ha, true).unwrap();
    session.set_auto_managed(&hb, true).unwrap();

    // Le dernier de la file (`b`, `queue_position` plus grande) doit
    // etre pause par la file ; `a` reste actif.
    wait_until(|| {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == hb)
            .map(|s| s.state == DownloadState::Paused)
            .unwrap_or(false)
    })
    .await;
    wait_until(|| {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == ha)
            .map(|s| s.state != DownloadState::Paused)
            .unwrap_or(false)
    })
    .await;

    // La pause de file n'est pas une pause utilisateur : `paused`
    // persiste a false en base.
    let ih_b = onionbit_crypto::hash::from_hex(&hb).unwrap();
    let db = onionbit_db::Database::open(&dir.path().join("onionbit.db")).unwrap();
    let paused_flag = db
        .with(|c| onionbit_db::downloads::get(c, &ih_b))
        .unwrap()
        .map(|r| r.paused)
        .unwrap_or(true);
    assert!(
        !paused_flag,
        "pause de file persistee comme pause utilisateur"
    );

    // Suppression de l'actif : le mis en file reprend au tick suivant.
    session.remove(&ha, false).await.unwrap();
    wait_until(|| {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == hb)
            .map(|s| s.state != DownloadState::Paused)
            .unwrap_or(false)
    })
    .await;

    session.stop().await;
}

/// Un telechargement non `auto_managed` n'entre jamais dans la file :
/// `active_downloads = 0` ne le pause pas.
#[tokio::test]
async fn queue_manager_ignore_les_non_auto_managed() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.queue.active_downloads = 0;
    cfg.queue.active_seeds = 0;
    cfg.queue.active_limit = 0;
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start");

    let a = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("a.bin", 42),
            false,
        )
        .await
        .unwrap();
    let ha = a.info_hash_hex();

    // Quelques ticks : jamais pause (pas `auto_managed`).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let s = session
        .downloads()
        .iter()
        .find(|s| s.info_hash == ha)
        .expect("download absent")
        .clone();
    assert_ne!(s.state, DownloadState::Paused, "non auto_managed pause");

    session.stop().await;
}

/// `POST /api/settings` (`apply_service_settings`) applique a chaud
/// les bornes de file (`set_session_limits` Python) : une baisse de
/// `active_downloads` pause l'excedent au tick suivant sans
/// redemarrage. Les limites de debit session passent aussi a chaud
/// (`Session::ratelimits` rqbit).
#[tokio::test]
async fn apply_service_settings_applique_les_bornes_a_chaud() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.queue.active_downloads = -1;
    cfg.queue.active_seeds = -1;
    cfg.queue.active_limit = -1;
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start");

    let a = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("a.bin", 42),
            false,
        )
        .await
        .unwrap();
    let b = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("b.bin", 42),
            false,
        )
        .await
        .unwrap();
    let (ha, hb) = (a.info_hash_hex(), b.info_hash_hex());
    session.set_auto_managed(&ha, true).unwrap();
    session.set_auto_managed(&hb, true).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_ne!(
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == hb)
            .map(|s| s.state),
        Some(DownloadState::Paused),
        "les deux doivent tourner sans borne"
    );

    // Modification a chaud : active_downloads 1 + debit session.
    let mut next = session.config().clone();
    next.queue.active_downloads = 1;
    next.engine.max_download_bps = Some(1024);
    next.engine.max_upload_bps = Some(2048);
    session.apply_service_settings(&next);

    // Le dernier de la file est pause des le tick suivant.
    wait_until(|| {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == hb)
            .map(|s| s.state == DownloadState::Paused)
            .unwrap_or(false)
    })
    .await;
    // GET /api/settings reflete la borne appliquee.
    assert_eq!(session.effective_config().queue.active_downloads, 1);
    // Limites rqbit appliquees a chaud sur le moteur.
    assert_eq!(session.engine().ratelimits(), (Some(2048), Some(1024)));

    // `saveas` modifie a chaud : le prochain ajout ecrit dans le
    // nouveau dossier (Python relit `saveas` a chaque ajout).
    let new_dir = dir.path().join("ailleurs");
    let mut next = session.config().clone();
    next.engine.output_dir = new_dir.clone();
    session.apply_service_settings(&next);
    let c = session
        .add_torrent_bytes(
            onionbit_test_support::test_torrent_bytes("c.bin", 42),
            false,
        )
        .await
        .unwrap();
    assert_eq!(c.output_folder(), new_dir, "saveas applique a chaud");

    session.stop().await;
}
