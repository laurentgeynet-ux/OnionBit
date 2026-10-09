// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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

/// Un telechargement anonyme (`anon_hops > 0`) dont le moteur est
/// indisponible a la restauration (ipv8 desactivee — config offline)
/// est saute ET signale au GUI via `tribler_exception`
/// (`on_tribler_exception` Python) : sans cette remontee la ligne DB
/// restait affichee « en verification » toute la session et tout
/// PATCH repondait 404.
#[tokio::test]
async fn restauration_anonyme_sans_ipv8_notifie_une_exception() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("anon.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    // Premiere session : ligne `downloads` anonyme persistee
    // directement (l'ajout anon reel exige la stack ipv8, absente en
    // offline — la ligne reproduit l'etat laisse par un run normal).
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .db()
        .with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &onionbit_db::DownloadRow {
                    infohash: onionbit_crypto::hash::from_hex(&ih).unwrap(),
                    name: Some("anon.bin".into()),
                    source_uri: format!("magnet:?xt=urn:btih:{ih}"),
                    torrent_data: Some(bytes),
                    anon_hops: 3,
                    ..Default::default()
                },
            )
        })
        .expect("upsert downloads");
    session.stop().await;

    // Seconde session : la restauration saute la ligne (pas de
    // moteur anonyme) et l'exception est diffusee.
    let notifier = Notifier::new();
    let mut rx = notifier.subscribe();
    let session = CoreSession::start(cfg, notifier).await.expect("start #2");
    session.wait_restored().await;
    assert!(session.find_download(&ih).is_none());
    assert!(session.restore_finished());

    let mut reported = false;
    while let Ok(n) = rx.try_recv() {
        if let onionbit_core::Notification::TriblerException { error } = n {
            if error.contains(&ih) {
                reported = true;
            }
        }
    }
    assert!(reported, "aucun tribler_exception pour {ih}");
    session.stop().await;
}

/// Regression : une ligne `downloads` sans metainfo persistee
/// (ajout magnet/URI — `persist` ne sauvegardait pas le metainfo
/// resolu dans `torrent_data`) devait re-resoudre le magnet a la
/// restauration. `resolve_magnet` (BEP 9) bloque indefiniment quand
/// aucun pair n'est joignable et figeait la file sequentielle : les
/// lignes suivantes restaient « en verification » et tout PATCH
/// repondait 404. Le re-add est desormais deporte en tache de fond
/// (visible en `pending`, statut METADATA) et la ligne reste
/// supprimable pendant la resolution.
#[tokio::test]
async fn restauration_magnet_non_resolu_ne_bloque_pas_la_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    // Un tracker injoignable garde le flux de pairs ouvert
    // (`TrackerComms` retente a l'infini) : la resolution BEP 9
    // attend sans fin au lieu d'echouer vite. `offline` desactive
    // les trackers — on les reactive pour reproduire le blocage.
    cfg.engine.disable_trackers = false;

    let ih_dead = "ab".repeat(20);
    let dead_uri = format!("magnet:?xt=urn:btih:{ih_dead}&tr=udp%3A%2F%2F127.0.0.1%3A9");
    let bytes = onionbit_test_support::test_torrent_bytes("ok.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih_ok = meta.info_hash_hex();

    // Premiere session : les lignes sont inserees directement —
    // le magnet sans `torrent_data` reproduit l'etat laisse par une
    // version qui ne persistait pas le metainfo resolu. La ligne
    // saine (avec `.torrent`) est inseree apres pour verifier qu'elle
    // n'est plus prise en otage par la ligne qui precede.
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .db()
        .with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &onionbit_db::DownloadRow {
                    infohash: onionbit_crypto::hash::from_hex(&ih_dead).unwrap(),
                    name: Some("dead.bin".into()),
                    source_uri: dead_uri,
                    ..Default::default()
                },
            )?;
            onionbit_db::downloads::upsert(
                c,
                &onionbit_db::DownloadRow {
                    infohash: onionbit_crypto::hash::from_hex(&ih_ok).unwrap(),
                    name: Some("ok.bin".into()),
                    source_uri: format!("magnet:?xt=urn:btih:{ih_ok}"),
                    torrent_data: Some(bytes),
                    ..Default::default()
                },
            )
        })
        .expect("upsert downloads");
    session.stop().await;

    // Seconde session : la restauration doit se terminer malgre le
    // magnet mort — avant le correctif `wait_restored` ne revenait
    // jamais.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    tokio::time::timeout(std::time::Duration::from_secs(30), session.wait_restored())
        .await
        .expect("restauration figee par un magnet non resolu");
    assert!(session.restore_finished());
    assert!(
        session.find_download(&ih_ok).is_some(),
        "ligne saine non restauree"
    );
    assert!(session.find_download(&ih_dead).is_none());
    assert!(
        session
            .pending_downloads()
            .iter()
            .any(|p| p.infohash == ih_dead),
        "magnet en resolution non expose en pending (METADATA)"
    );

    // Un download en resolution reste supprimable (etat METADATA
    // Python) : entree pending retiree + ligne persistee effacee.
    session
        .remove(&ih_dead, false)
        .await
        .expect("remove pendant la resolution");
    assert!(session.pending_downloads().is_empty());
    let row = session
        .db()
        .with(|c| {
            onionbit_db::downloads::get(c, &onionbit_crypto::hash::from_hex(&ih_dead).unwrap())
        })
        .expect("get downloads");
    assert!(row.is_none(), "ligne persistee non supprimee");
    session.stop().await;
}

/// `.torrent` mono-fichier a hash de piece REEL : la fixture
/// `test_torrent_bytes` (hash nuls) ne peut jamais etre complete ;
/// ici le fichier ecrit dans le dossier de sortie valide le
/// hash-check initial de rqbit.
fn torrent_complet(name: &str, content: &[u8]) -> Vec<u8> {
    use onionbit_format::bencode::{encode, BValue};
    let mut info = std::collections::BTreeMap::new();
    info.insert(b"length".to_vec(), BValue::Int(content.len() as i64));
    info.insert(b"name".to_vec(), BValue::Bytes(name.as_bytes().to_vec()));
    info.insert(b"piece length".to_vec(), BValue::Int(16384));
    info.insert(
        b"pieces".to_vec(),
        BValue::Bytes(onionbit_crypto::hash::sha1(content).to_vec()),
    );
    let mut root = std::collections::BTreeMap::new();
    root.insert(b"info".to_vec(), BValue::Dict(info));
    encode(&BValue::Dict(root))
}

/// Regression : un telechargement restaure deja termine ne doit PAS
/// redeclencher `torrent_finished` au demarrage — l'alerte libtorrent
/// ne se rejoue pas au chargement d'un checkpoint. Sans le
/// pre-amorcage du set `finished` depuis le drapeau persistant en
/// base, chaque lancement re-notifiait tous les telechargements
/// complets (et relancait leur `check_after_complete`).
#[tokio::test]
async fn torrent_fini_restaure_ne_renotifie_pas() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;

    let content = b"onionbit finished payload".to_vec();
    let bytes = torrent_complet("done.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();
    let ih_bin = || onionbit_crypto::hash::from_hex(&ih).unwrap();

    // Premiere session : les donnees sont deja sur disque — le
    // hash-check initial complete le torrent, une notification est
    // attendue (completion de CETTE session).
    let notifier = Notifier::new();
    let mut rx1 = notifier.subscribe();
    let session = CoreSession::start(cfg.clone(), notifier)
        .await
        .expect("start #1");
    std::fs::create_dir_all(&cfg.engine.output_dir).unwrap();
    std::fs::write(cfg.engine.output_dir.join("done.bin"), &content).unwrap();
    let dl = session
        .add_torrent_bytes(bytes, false)
        .await
        .expect("add torrent");
    assert_eq!(dl.info_hash_hex(), ih);

    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == ih)
            .map(|s| s.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(done, "torrent complet sur disque jamais vu finished");
    // Attendre le tick qui persiste le drapeau `finished` (la
    // notification precede l'ecriture dans le meme bloc).
    let flagged = onionbit_test_support::wait_for(std::time::Duration::from_secs(5), || {
        session
            .db()
            .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
            .ok()
            .flatten()
            .map(|r| r.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(flagged, "drapeau finished non persiste");

    let mut notified = 0;
    while let Ok(n) = rx1.try_recv() {
        if let onionbit_core::Notification::DownloadFinished { infohash, .. } = n {
            if infohash == ih {
                notified += 1;
            }
        }
    }
    assert_eq!(notified, 1, "completion de session non notifiee");
    session.stop().await;

    // Seconde session : la restauration retrouve les donnees, le
    // hash-check re-complete le torrent — mais aucune notification ne
    // doit repartir (drapeau `finished` persistant).
    let notifier = Notifier::new();
    let mut rx2 = notifier.subscribe();
    let session = CoreSession::start(cfg, notifier).await.expect("start #2");
    session.wait_restored().await;
    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == ih)
            .map(|s| s.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(done, "torrent restaure jamais revenu finished");
    // Quelques ticks supplementaires : la notification eventuelle a eu
    // le temps d'etre emise.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let mut notified = 0;
    while let Ok(n) = rx2.try_recv() {
        if let onionbit_core::Notification::DownloadFinished { infohash, .. } = n {
            if infohash == ih {
                notified += 1;
            }
        }
    }
    assert_eq!(
        notified, 0,
        "torrent_finished re-emis pour un telechargement deja termine"
    );
    session.stop().await;
}

/// Etat « fichiers manquants » (parite qBittorrent `MissingFiles`) :
/// un telechargement termine dont le contenu a ete supprime pendant
/// l'arret est restaure marque + pause — JAMAIS re-telecharge en
/// douce dans le dossier final. La ligne persiste : l'utilisateur
/// choisit ensuite suppression ou re-telechargement explicite.
#[tokio::test]
async fn fichier_final_supprime_restaure_marque_manquant() {
    use onionbit_bittorrent::DownloadState;

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;

    let content = b"contenu supprime pendant l'arret".to_vec();
    let bytes = torrent_complet("gone.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();
    let ih_bin = || onionbit_crypto::hash::from_hex(&ih).unwrap();

    // Session 1 : contenu complet sur disque → `finished` persiste.
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    std::fs::create_dir_all(&cfg.engine.output_dir).unwrap();
    let file = cfg.engine.output_dir.join("gone.bin");
    std::fs::write(&file, &content).unwrap();
    session.add_torrent_bytes(bytes, false).await.unwrap();
    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .db()
            .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
            .ok()
            .flatten()
            .map(|r| r.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(done, "drapeau finished non persiste");
    session.stop().await;

    // Suppression manuelle du fichier termine pendant l'arret.
    std::fs::remove_file(&file).unwrap();

    // Session 2 : la ligne est restauree marquee « fichiers manquants »
    // et pausee — visible dans `downloads()` (Error + motif), pas
    // silencieusement relancee.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let restored = session
        .find_download(&ih)
        .unwrap_or_else(|| panic!("download {ih} non restaure"));
    assert!(restored.is_paused(), "download manquant non pause");
    let stats = session
        .downloads()
        .into_iter()
        .find(|s| s.info_hash == ih)
        .expect("stats manquantes");
    assert_eq!(stats.state, DownloadState::Error, "etat expose: {stats:?}");
    assert!(
        stats.error.as_deref().unwrap_or("").contains("manquant"),
        "motif absent: {:?}",
        stats.error
    );
    // La ligne reste en base (choix laisse a l'utilisateur) mais le
    // drapeau `finished` est retombe — plus de « termine » fictif.
    let row = session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
        .expect("get downloads")
        .expect("ligne supprimee");
    assert!(!row.finished);

    // Suppression explicite : ligne + marque purges, liste vide.
    session.remove(&ih, true).await.expect("remove");
    assert!(session.downloads().is_empty());
    assert!(
        session
            .db()
            .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
            .expect("get downloads")
            .is_none(),
        "ligne persistee non supprimee"
    );
    session.stop().await;
}

/// Reprise explicite d'un download « fichiers manquants » toujours
/// absent : le telechargement repart en `temp` (move_on_completion
/// le refoulera a la destination finale) — jamais dans le dossier
/// final, parite qBittorrent + layout temp/downloads Tribler.
#[tokio::test]
async fn resume_manquant_rebascule_en_temp() {
    use onionbit_bittorrent::DownloadState;

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.storage.move_on_completion = true;
    let roots = onionbit_core::paths::PathRoots::for_state_dir(dir.path());
    let temp = roots.public_temp();
    let downloads = roots.public_downloads();

    let content = b"payload manquant retelecharge en temp".to_vec();
    let bytes = torrent_complet("relocate.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    // Session 1 : contenu complet en temp → completion →
    // move_on_completion le range en `downloads`.
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("relocate.bin"), &content).unwrap();
    session.add_torrent_bytes(bytes, false).await.unwrap();
    let moved = onionbit_test_support::wait_for(std::time::Duration::from_secs(15), || {
        downloads.join("relocate.bin").is_file()
    })
    .await;
    assert!(moved, "move_on_completion n'a pas range le fichier");
    session.stop().await;

    // Suppression du fichier final pendant l'arret.
    std::fs::remove_file(downloads.join("relocate.bin")).unwrap();

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let restored = session.find_download(&ih).expect("non restaure");
    assert!(restored.is_paused(), "manquant non pause a la restauration");
    // La restauration ne doit pas avoir recree de stub : la lecture
    // paresseuse du hashcheck ne materialise plus les fichiers absents.
    assert!(
        !downloads.join("relocate.bin").exists(),
        "stub recree par le check de restauration"
    );

    // Reprise explicite : repart en `temp` (pas de re-download en
    // douce dans `downloads`), la marque saute.
    session.resume(&ih).await.expect("resume manquant");
    let dl = session.find_download(&ih).expect("download absent");
    assert!(
        dl.output_folder().starts_with(&temp),
        "reprise hors de temp : {:?} (attendu sous {:?})",
        dl.output_folder(),
        temp
    );
    let stats = session
        .downloads()
        .into_iter()
        .find(|s| s.info_hash == ih)
        .expect("stats manquantes");
    assert_ne!(
        stats.state,
        DownloadState::Error,
        "encore expose en erreur: {:?}",
        stats.error
    );
    session.stop().await;
}

/// Reprise d'un download « manquant » dont le contenu est revenu :
/// reprise en place (pas de relocalisation), le hashcheck a la
/// reprise retrouve les pieces et le torrent re-termine.
#[tokio::test]
async fn resume_manquant_fichiers_revenus_reprend_en_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;

    let content = b"contenu retrouve".to_vec();
    let bytes = torrent_complet("back.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    // Session 1 : contenu complet → `finished` persiste.
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    std::fs::create_dir_all(&cfg.engine.output_dir).unwrap();
    let file = cfg.engine.output_dir.join("back.bin");
    std::fs::write(&file, &content).unwrap();
    session.add_torrent_bytes(bytes, false).await.unwrap();
    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == ih)
            .map(|s| s.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(done, "jamais finished");
    session.stop().await;

    // Deplacement manuel du fichier → restauration « manquant ».
    let stash = dir.path().join("stash.bin");
    std::fs::rename(&file, &stash).unwrap();
    let out_dir = cfg.engine.output_dir.clone();
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let stats = session
        .downloads()
        .into_iter()
        .find(|s| s.info_hash == ih)
        .expect("stats manquantes");
    assert!(
        stats.error.as_deref().unwrap_or("").contains("manquant"),
        "non marque manquant: {stats:?}"
    );

    // Le fichier revient → reprise en place, pas de bascule en temp.
    std::fs::rename(&stash, &file).unwrap();
    session.resume(&ih).await.expect("resume");
    let dl = session.find_download(&ih).expect("download absent");
    assert_eq!(
        dl.output_folder()
            .components()
            .collect::<std::path::PathBuf>(),
        out_dir.components().collect::<std::path::PathBuf>(),
        "relocalise alors que le contenu est revenu"
    );
    let again = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .downloads()
            .iter()
            .find(|s| s.info_hash == ih)
            .map(|s| s.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(again, "jamais revenu finished apres reprise");
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

/// Regression : un magnet en resolution (`pending`, statut METADATA)
/// n'a pas d'objet moteur — `find_download_hex` repondait donc 404 a
/// `PATCH anon_hops`, pause et suppression, alors que le download etait
/// visible dans la liste. Un re-`PUT` du meme magnet spawnait une
/// seconde tache de resolution : les deux materialisaient un download
/// moteur pour le meme infohash (lignes doublees ; l'orphelin restant
/// apres suppression ressortait en lane par defaut, badge « Clair »).
#[tokio::test]
async fn operations_sur_magnet_en_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    // Tracker injoignable : la resolution BEP 9 attend sans fin, le
    // download reste en `pending` tout le long du test.
    cfg.engine.disable_trackers = false;

    let ih = "cd".repeat(20);
    let uri = format!("magnet:?xt=urn:btih:{ih}&tr=udp%3A%2F%2F127.0.0.1%3A9");

    // Ligne persistee sans metainfo → restauration differee (pending).
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .db()
        .with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &onionbit_db::DownloadRow {
                    infohash: onionbit_crypto::hash::from_hex(&ih).unwrap(),
                    name: Some("pending.bin".into()),
                    source_uri: uri.clone(),
                    ..Default::default()
                },
            )
        })
        .expect("upsert downloads");
    session.stop().await;

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    tokio::time::timeout(std::time::Duration::from_secs(30), session.wait_restored())
        .await
        .expect("restauration figee");
    let pending_seen = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session.pending_downloads().iter().any(|p| p.infohash == ih)
    })
    .await;
    assert!(pending_seen, "magnet en resolution absent du pending");

    // `PATCH anon_hops` sur le pending : accepte (meme lane → no-op)
    // au lieu du 404 precedent ; les bornes restent appliquees et
    // une lane anonyme sans stack ipv8 est refusee proprement.
    session
        .update_hops(&ih, 0)
        .await
        .expect("update_hops pending");
    assert!(
        session.update_hops(&ih, 4).await.is_err(),
        "anon_hops > MAX accepte sur pending"
    );
    assert!(
        session.update_hops(&ih, 2).await.is_err(),
        "lane anonyme acceptee sans stack ipv8"
    );

    // Re-`PUT` du meme magnet : refuse (`download_exists` etendu a
    // METADATA) au lieu de respawner une seconde resolution.
    let dup = session.add_download_anon(&uri, false, 0, false, None).await;
    assert!(
        matches!(dup, Err(onionbit_core::CoreError::InvalidState(_))),
        "re-add d'un magnet en resolution accepte : {dup:?}"
    );

    // Pause/reprise sur le pending : portees par `pending.paused` et
    // la ligne persistee (le Download METADATA Python est pausable).
    session.pause(&ih).await.expect("pause pending");
    let paused = session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &onionbit_crypto::hash::from_hex(&ih).unwrap()))
        .expect("get")
        .map(|r| r.paused)
        .unwrap_or(false);
    assert!(paused, "pause pending non persistee");
    session.resume(&ih).await.expect("resume pending");

    // Suppression pendant la resolution : entree retiree, ligne
    // effacee, tache de resolution reveillee puis abandonnee.
    session.remove(&ih, false).await.expect("remove pending");
    assert!(session.pending_downloads().is_empty());
    let row = session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &onionbit_crypto::hash::from_hex(&ih).unwrap()))
        .expect("get downloads");
    assert!(row.is_none(), "ligne persistee non supprimee");
    session.stop().await;
}

/// ADR-0018 etape 59 : `storage/move_on_completion` — un ajout sans
/// `destination` ecrit dans `data/public/temp` ; a la transition
/// `finished` le contenu declare (et lui seul) est refoule vers
/// `data/public/downloads`, `output_dir` persistee devient
/// `@public/downloads`, et le redemarrage re-hash le torrent fini
/// a son nouvel emplacement (re-check rqbit de reconnaissance).
#[tokio::test]
async fn move_on_completion_deplace_temp_vers_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.storage.move_on_completion = true;
    let roots = onionbit_core::paths::PathRoots::for_state_dir(dir.path());
    let temp = roots.public_temp();
    let downloads = roots.public_downloads();

    let content = b"payload move_on_completion".to_vec();
    let bytes = torrent_complet("moved.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();
    let ih_bin = || onionbit_crypto::hash::from_hex(&ih).unwrap();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    // Contenu complet deja en place + un fichier etranger qui ne doit
    // PAS suivre (durcissement : fichiers declares seulement).
    std::fs::write(temp.join("moved.bin"), &content).unwrap();
    std::fs::write(temp.join("etranger.txt"), b"pas a deplacer").unwrap();
    let dl = session
        .add_torrent_bytes(bytes, false)
        .await
        .expect("add torrent");
    assert_eq!(
        dl.output_folder()
            .components()
            .collect::<std::path::PathBuf>(),
        temp.components().collect::<std::path::PathBuf>(),
        "ajout sans destination doit ecrire dans public/temp"
    );

    // Completion detectee par le progress loop → deplacement.
    let moved = onionbit_test_support::wait_for(std::time::Duration::from_secs(15), || {
        downloads.join("moved.bin").is_file()
    })
    .await;
    assert!(moved, "contenu termine non refoule vers public/downloads");
    assert!(
        !temp.join("moved.bin").exists(),
        "source encore presente en temp"
    );
    assert!(
        temp.join("etranger.txt").is_file(),
        "fichier etranger au torrent deplace par erreur"
    );

    // `output_dir` persistee en spec portable vers la zone finale.
    let persisted = onionbit_test_support::wait_for(std::time::Duration::from_secs(5), || {
        session
            .db()
            .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
            .ok()
            .flatten()
            .map(|r| r.output_dir == "@public/downloads")
            .unwrap_or(false)
    })
    .await;
    assert!(
        persisted,
        "output_dir persistee non mise a jour vers @public/downloads"
    );
    session.stop().await;

    // Redemarrage : la restauration relit `@public/downloads` et le
    // re-hash rqbit reconnait le contenu deja complet.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .find_download(&ih)
            .map(|d| d.stats().finished)
            .unwrap_or(false)
    })
    .await;
    assert!(done, "torrent deplace jamais revenu finished au restart");
    let restored = session.find_download(&ih).unwrap();
    assert_eq!(
        restored
            .output_folder()
            .components()
            .collect::<std::path::PathBuf>(),
        downloads.components().collect::<std::path::PathBuf>(),
    );
    session.stop().await;
}

/// `storage/move_on_completion = false` : comportement historique —
/// l'ajout ecrit directement dans le dossier final, rien ne bouge a
/// la completion.
#[tokio::test]
async fn move_on_completion_desactive_garde_la_destination() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.storage.move_on_completion = false;
    let roots = onionbit_core::paths::PathRoots::for_state_dir(dir.path());
    let downloads = roots.public_downloads();

    let content = b"payload sans move".to_vec();
    let bytes = torrent_complet("stay.bin", &content);

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("stay.bin"), &content).unwrap();
    let dl = session
        .add_torrent_bytes(bytes, false)
        .await
        .expect("add torrent");
    assert_eq!(
        dl.output_folder()
            .components()
            .collect::<std::path::PathBuf>(),
        downloads.components().collect::<std::path::PathBuf>(),
        "move_on_completion=false doit garder le dossier final"
    );
    let done = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session.downloads().iter().any(|s| s.finished)
    })
    .await;
    assert!(done);
    // Quelques ticks : un deplacement intempestif aurait eu lieu.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        downloads.join("stay.bin").is_file(),
        "contenu deplace malgre move_on_completion=false"
    );
    session.stop().await;
}

/// Echec du deplacement (destination `public/downloads` inutilisable) :
/// le contenu reste en `temp`, `output_dir` persistee inchangee, et
/// l'echec est signale via `tribler_exception` — jamais de perte.
#[tokio::test]
async fn move_on_completion_echec_conserve_le_contenu_en_temp() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.storage.move_on_completion = true;
    let roots = onionbit_core::paths::PathRoots::for_state_dir(dir.path());
    let temp = roots.public_temp();
    let downloads = roots.public_downloads();

    let content = b"payload bloque".to_vec();
    let bytes = torrent_complet("blocked.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();
    let ih_bin = || onionbit_crypto::hash::from_hex(&ih).unwrap();

    let notifier = Notifier::new();
    let mut rx = notifier.subscribe();
    let session = CoreSession::start(cfg, notifier).await.expect("start");
    // `public/downloads` est bloquee APRES le boot (le moteur exige un
    // `output_dir` valide au demarrage) : le dossier est remplace par
    // un fichier ordinaire — `move_storage` refusera la cible.
    std::fs::remove_dir(&downloads).expect("remove downloads dir");
    std::fs::write(&downloads, b"blocked").unwrap();
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("blocked.bin"), &content).unwrap();
    session
        .add_torrent_bytes(bytes, false)
        .await
        .expect("add torrent");

    // La completion arrive (le hash-check reussit dans temp), le move
    // echoue — attendre la notification d'echec.
    let flagged = onionbit_test_support::wait_for(std::time::Duration::from_secs(15), || {
        session
            .db()
            .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
            .ok()
            .flatten()
            .map(|r| r.finished)
            .unwrap_or(false)
    })
    .await;
    assert!(flagged, "completion jamais detectee");
    // `wait_for` rappelle `f` une derniere fois apres sa boucle : un
    // predicat non idempotent (ici le drainage du canal) doit memoriser
    // sa reussite dans un flag, jamais la retourner directement.
    let mut reported = false;
    let seen = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        while let Ok(n) = rx.try_recv() {
            if let onionbit_core::Notification::TriblerException { error } = n {
                if error.contains("move_on_completion") {
                    reported = true;
                }
            }
        }
        reported
    })
    .await;
    assert!(seen, "echec du deplacement jamais signale");

    assert!(
        temp.join("blocked.bin").is_file(),
        "contenu perdu en cas d'echec du deplacement"
    );
    let out = session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &ih_bin()))
        .expect("get")
        .map(|r| r.output_dir)
        .unwrap_or_default();
    assert_eq!(
        out, "@public/temp",
        "output_dir persistee modifiee malgre l'echec"
    );
    session.stop().await;
}

/// Reprise : une ligne `finished` encore en `@public/temp` (kill entre
/// la completion et le rangement) est refoulee au redemarrage — le
/// set `finished` du progress loop est pre-amorce et ne rejouerait
/// jamais le deplacement.
#[tokio::test]
async fn move_on_completion_rejoue_au_redemarrage_si_fini_en_temp() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.progress_interval_ms = 50;
    cfg.storage.move_on_completion = true;
    let roots = onionbit_core::paths::PathRoots::for_state_dir(dir.path());
    let temp = roots.public_temp();
    let downloads = roots.public_downloads();

    let content = b"payload reprise".to_vec();
    let bytes = torrent_complet("orphan.bin", &content);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    // Etat laisse par un kill post-completion : ligne `finished` avec
    // `output_dir = @public/temp`, contenu encore sur place.
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("orphan.bin"), &content).unwrap();
    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .db()
        .with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &onionbit_db::DownloadRow {
                    infohash: onionbit_crypto::hash::from_hex(&ih).unwrap(),
                    name: Some("orphan.bin".into()),
                    source_uri: format!("magnet:?xt=urn:btih:{ih}"),
                    torrent_data: Some(bytes),
                    output_dir: "@public/temp".into(),
                    finished: true,
                    ..Default::default()
                },
            )
        })
        .expect("upsert downloads");
    session.stop().await;

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let moved = onionbit_test_support::wait_for(std::time::Duration::from_secs(15), || {
        downloads.join("orphan.bin").is_file()
    })
    .await;
    assert!(
        moved,
        "torrent fini reste en temp n'a pas ete range au redemarrage"
    );
    session.stop().await;
}
