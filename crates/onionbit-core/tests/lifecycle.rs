//! Cycle de vie du daemon : persistance des telechargements entre
//! deux `CoreSession` sur le meme `state_dir` (equivalent du
//! redemarrage Tribler — `downloads` + fastresume rqbit).
//!
//! Hors-ligne : config `offline` (aucun trafic), base fichier reelle
//! pour tester la persistance.

use onionbit_core::{CoreConfig, CoreSession, Notifier};

/// Redemarrage : un telechargement ajoute a la premiere session doit
/// etre restaure dans la seconde (meme info-hash, etat pause
/// conserve).
#[tokio::test]
async fn downloads_restores_apres_redemarrage() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("cycle.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
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

    // Seconde session : restauration depuis `downloads` (DB fichier) —
    // tache de fond (`load_checkpoint` Python), on attend sa fin.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
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
    let bytes = onionbit_test_support::test_torrent_bytes("gone.bin", 42);

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    let dl = session.add_torrent_bytes(bytes, true).await.unwrap();
    session.remove(&dl.info_hash_hex(), false).await.unwrap();
    session.stop().await;

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    assert!(session.downloads().is_empty(), "download supprime restaure");
    session.stop().await;
}

/// Stockage paresseux : un dossier de sortie devenu inaccessible
/// entre deux runs n'est plus detecte a l'ajout (init sans acces
/// disque) — l'erreur est differee a la premiere E/S. Le check
/// fastresume/recheck doit alors marquer les pieces manquantes sans
/// paniquer ni laisser le download en etat d'erreur fatale, et
/// l'etat expose doit rester intelligible.
#[tokio::test]
async fn restauration_sortie_inaccessible_erreur_differee() {
    use onionbit_bittorrent::DownloadState;

    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("cycle.bin", 42);

    // Premiere session : ajout actif dans une destination dediee
    // (sinon `output_folder` = le dossier `downloads` de l'engine, dont
    // le blocage empecherait carrement le second demarrage — ce n'est
    // pas le scenario vise).
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    let dest = dir.path().join("dest");
    let dl = session
        .add_torrent_bytes_anon(bytes, false, 0, false, Some(dest))
        .await
        .expect("add torrent");
    let ih = dl.info_hash_hex();
    // `components()` normalise le separateur final de output_folder.
    let out: std::path::PathBuf = dl.output_folder().components().collect();
    session.stop().await;

    // Le dossier de sortie est remplace par un fichier ordinaire :
    // toute ouverture paresseuse (`create_dir_all` sur un parent qui
    // est un fichier) echouera a la premiere lecture/ecriture.
    let _ = std::fs::remove_dir_all(&out);
    std::fs::write(&out, b"blocked").unwrap();

    // Seconde session : la restauration elle-meme doit reussir
    // (init lazy sans disque), puis le check differe consomme l'erreur.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let restored = session
        .find_download(&ih)
        .unwrap_or_else(|| panic!("download {ih} non restaure"));

    // Le check peut etre encore en cours (etat Checking) : attendre
    // qu'il sorte de Initializing/Checking.
    let settled = onionbit_test_support::wait_for(std::time::Duration::from_secs(15), || {
        let s = restored.stats();
        !matches!(
            s.state,
            DownloadState::Initializing | DownloadState::Checking
        )
    })
    .await;
    let s = restored.stats();
    assert!(
        settled,
        "le check differe n'a pas termine, etat: {:?}",
        s.state
    );
    assert_ne!(
        s.state,
        DownloadState::Error,
        "erreur fatale inattendue a la premiere E/S: {:?}",
        s.error
    );
    // Aucune donnee : toutes les pieces sont marquees manquantes.
    assert_eq!(s.progress_bytes, 0);
    session.stop().await;
}

/// `pause_all`/`resume_all` : tous les telechargements d'une session
/// basculent ensemble (suspension mobile / arret rapide — etape 19).
#[tokio::test]
async fn pause_all_resume_all_basculent_tous_les_telechargements() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
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
        .add_torrent_bytes(onionbit_test_support::test_torrent_bytes("b.bin", 42), true)
        .await
        .unwrap();
    assert!(!a.is_paused());
    assert!(b.is_paused());

    let errors = session.pause_all().await;
    assert!(errors.is_empty(), "pause_all : {errors:?}");
    assert!(a.is_paused());
    assert!(b.is_paused());

    let errors = session.resume_all().await;
    assert!(errors.is_empty(), "resume_all : {errors:?}");
    assert!(!a.is_paused());
    assert!(!b.is_paused());

    session.stop().await;
}
