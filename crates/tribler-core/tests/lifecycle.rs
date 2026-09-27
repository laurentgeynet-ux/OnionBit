//! Cycle de vie du daemon : persistance des telechargements entre
//! deux `CoreSession` sur le meme `state_dir` (equivalent du
//! redemarrage Tribler — `downloads` + fastresume rqbit).
//!
//! Hors-ligne : config `offline` (aucun trafic), base fichier reelle
//! pour tester la persistance.

use tribler_core::{CoreConfig, CoreSession, Notifier};

/// Redemarrage : un telechargement ajoute a la premiere session doit
/// etre restaure dans la seconde (meme info-hash, etat pause
/// conserve).
#[tokio::test]
async fn downloads_restores_apres_redemarrage() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = tribler_test_support::test_torrent_bytes("cycle.bin", 42);
    let meta = tribler_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    // Premiere session : ajout en pause.
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    let dl = session
        .add_torrent_bytes(bytes, true)
        .await
        .expect("add torrent");
    assert!(dl.is_paused());
    session.stop().await;

    // Seconde session : restauration depuis `downloads` (DB fichier).
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    let restored = session
        .find_download(&ih)
        .unwrap_or_else(|| panic!("download {ih} non restaure"));
    assert_eq!(restored.info_hash_hex(), ih);
    assert!(restored.is_paused(), "etat pause non restaure");
    session.stop().await;
}

/// La suppression d'un telechargement le retire aussi de la
/// persistance : apres redemarrage il n'est pas restaure.
#[tokio::test]
async fn download_supprime_n_est_pas_restaure() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = tribler_test_support::test_torrent_bytes("gone.bin", 42);

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    let dl = session.add_torrent_bytes(bytes, true).await.unwrap();
    session.remove(&dl.info_hash_hex(), false).await.unwrap();
    session.stop().await;

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    assert!(session.downloads().is_empty(), "download supprime restaure");
    session.stop().await;
}
