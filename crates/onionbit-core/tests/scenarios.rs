// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scenarios applicatifs de gestion des telechargements — le niveau
//! auquel un utilisateur opere dans l'UI (l'ajout via le moteur de
//! recherche interne produit exactement le meme `add_download_anon`
//! magnet, le dialogue ne fait que construire l'URI) :
//!
//! - ajout sur chaque lane `hops = 0/1/2/3` ;
//! - migration de lane en cours (`PATCH anon_hops` sur un download
//!   materialise et sur un magnet en resolution `METADATA`) ;
//! - re-add du meme download (materialise puis en cours de
//!   resolution) — dedup `download_exists` ;
//! - pause/reprise et suppression dans les deux etats ;
//! - restauration sur la bonne lane au redemarrage.
//!
//! Hors-ligne : stack IPv8 + anonymat actifs en loopback (endpoint
//! ephemere, pas de bootstrap) — les lanes anonymes se creent a la
//! demande via `engine_for`. Aucun relais distant : les circuits ne
//! montent pas, le kill switch reste engage — parfait pour tester la
//! logique sans dependre d'un vrai transfert.

use std::sync::Arc;

use onionbit_core::{CoreConfig, CoreSession, Notifier};

/// Session loopback avec anonymat : les lanes `anon_engine(1..=3)`
/// peuvent etre creees sans aucun pair distant.
async fn start_anon_session(dir: &std::path::Path) -> Arc<CoreSession> {
    let mut cfg = CoreConfig::offline(dir.to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    Arc::new(
        CoreSession::start(cfg, Notifier::new())
            .await
            .expect("start session"),
    )
}

fn row_anon_hops(session: &CoreSession, ih: &str) -> i64 {
    session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &onionbit_crypto::hash::from_hex(ih).unwrap()))
        .expect("get downloads")
        .map(|r| r.anon_hops)
        .unwrap_or(-999)
}

/// Ajout direct sur chaque lane (equivalent des choix « Clair » /
/// « Anon xN » du dialogue) : le download materialise sur le moteur
/// de la lane demandee et la lane est persistee.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ajouts_sur_les_quatre_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let session = start_anon_session(dir.path()).await;

    for hops in 0..=3u32 {
        let bytes = onionbit_test_support::test_torrent_bytes(
            &format!("lane{hops}.bin"),
            42 + u64::from(hops),
        );
        let dl = session
            .add_torrent_bytes_anon(bytes, true, hops, true, None)
            .await
            .expect("add_torrent_bytes_anon");
        let ih = dl.info_hash_hex();
        assert_eq!(
            session.owner_engine_hops(&ih),
            Some(hops),
            "lane moteur incorrecte pour hops={hops}"
        );
        assert_eq!(
            row_anon_hops(&session, &ih),
            i64::from(hops),
            "lane persistee incorrecte pour hops={hops}"
        );
    }
    session.stop().await;
}

/// Migration de lane en cours (`update_hops` sur un download
/// materialise) : le download quitte son moteur pour la nouvelle lane
/// et la ligne persistee suit — y compris le retour en clair (choix
/// explicite) et le re-depart vers l'anonyme.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migration_de_lane_en_cours() {
    let dir = tempfile::tempdir().unwrap();
    let session = start_anon_session(dir.path()).await;

    let bytes = onionbit_test_support::test_torrent_bytes("move.bin", 42);
    let dl = session
        .add_torrent_bytes_anon(bytes, false, 3, true, None)
        .await
        .expect("add x3");
    let ih = dl.info_hash_hex();
    assert_eq!(session.owner_engine_hops(&ih), Some(3));

    for target in [1u32, 0, 2] {
        session.update_hops(&ih, target).await.expect("update_hops");
        assert_eq!(
            session.owner_engine_hops(&ih),
            Some(target),
            "download pas sur la lane {target} apres migration"
        );
        assert_eq!(row_anon_hops(&session, &ih), i64::from(target));
    }
    // Une seule ligne pour le meme infohash, quelles que soient les
    // migrations (le doublon historique etait liste deux fois).
    let count = session
        .downloads()
        .iter()
        .filter(|s| s.info_hash == ih)
        .count();
    assert_eq!(count, 1, "infohash liste en double apres migrations");
    session.stop().await;
}

/// Dedup `download_exists` : le second ajout d'un download
/// materialise retourne l'existant (la lane de la SEULECONDE demande
/// ne doit PAS migrer l'existant — c'est un no-op, pas un PATCH) ;
/// un re-`PUT` pendant la resolution magnet est refuse au lieu de
/// respawner une seconde tache (le scenario qui produisait les
/// lignes doublees observees en reel).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doublons_refuses_materialise_et_pending() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    // Trackers reactives : la resolution magnet attend un pair
    // injoignable → le download reste en `pending` (METADATA).
    cfg.engine.disable_trackers = false;
    let session = Arc::new(
        CoreSession::start(cfg, Notifier::new())
            .await
            .expect("start"),
    );

    // 1. Materialise : le re-add retourne l'existant, lane inchangee.
    let bytes = onionbit_test_support::test_torrent_bytes("dup.bin", 42);
    let d1 = session
        .add_torrent_bytes_anon(bytes.clone(), false, 1, true, None)
        .await
        .expect("add x1");
    let d2 = session
        .add_torrent_bytes_anon(bytes, false, 3, true, None)
        .await
        .expect("re-add x3");
    assert_eq!(d1.info_hash_hex(), d2.info_hash_hex());
    assert_eq!(
        session.owner_engine_hops(&d1.info_hash_hex()),
        Some(1),
        "le re-add ne doit pas migrer l'existant"
    );

    // 2. Pending : le re-add est refuse (`InvalidState`) — pas de
    // seconde tache de resolution en vol.
    let ih_dead = "ef".repeat(20);
    let uri = format!("magnet:?xt=urn:btih:{ih_dead}&tr=udp%3A%2F%2F127.0.0.1%3A9");
    let s = session.clone();
    let u = uri.clone();
    let add_task = tokio::spawn(async move { s.add_download_anon(&u, false, 3, true, None).await });
    let seen = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session
            .pending_downloads()
            .iter()
            .any(|p| p.infohash == ih_dead)
    })
    .await;
    assert!(seen, "magnet jamais passe en pending");

    let dup = session.add_download_anon(&uri, false, 2, true, None).await;
    assert!(
        matches!(dup, Err(onionbit_core::CoreError::InvalidState(_))),
        "re-add d'un magnet en resolution accepte : {dup:?}"
    );

    // Nettoyage : la suppression reveille la tache en vol.
    session
        .remove(&ih_dead, false)
        .await
        .expect("remove pending");
    let res = add_task.await.expect("task paniquee");
    assert!(
        matches!(res, Err(onionbit_core::CoreError::Cancelled(_))),
        "la tache aurait du abandonner silencieusement : {res:?}"
    );
    session.stop().await;
}

/// Le scenario reel observe : magnet en resolution `METADATA` →
/// `PATCH anon_hops` accepte (la tache repart sur la lane demandee),
/// pause prise en compte, suppression propre qui reveille et termine
/// la tache sans download resuscite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn patch_lane_sur_magnet_en_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    cfg.engine.disable_trackers = false;
    let session = Arc::new(
        CoreSession::start(cfg, Notifier::new())
            .await
            .expect("start"),
    );

    let ih = "ab".repeat(20);
    let uri = format!("magnet:?xt=urn:btih:{ih}&dn=stuck&tr=udp%3A%2F%2F127.0.0.1%3A9");
    let s = session.clone();
    let u = uri.clone();
    let add_task = tokio::spawn(async move { s.add_download_anon(&u, false, 3, true, None).await });
    let seen = onionbit_test_support::wait_for(std::time::Duration::from_secs(10), || {
        session.pending_downloads().iter().any(|p| p.infohash == ih)
    })
    .await;
    assert!(seen, "magnet jamais passe en pending");

    // PATCH x3 -> x2 : accepte (plus de 404), la cible pending suit.
    session.update_hops(&ih, 2).await.expect("patch pending");
    let target = onionbit_test_support::wait_for(std::time::Duration::from_secs(5), || {
        session
            .pending_downloads()
            .iter()
            .find(|p| p.infohash == ih)
            .map(|p| p.anon_hops == 2)
            .unwrap_or(false)
    })
    .await;
    assert!(target, "cible pending non retouchee a x2");

    // Pause sur le pending : portee par pending.paused.
    session.pause(&ih).await.expect("pause pending");
    let paused = session
        .pending_downloads()
        .iter()
        .find(|p| p.infohash == ih)
        .map(|p| p.paused)
        .unwrap_or(false);
    assert!(paused, "pause non portee par pending");

    // Suppression : pending retire, tache terminee en Cancelled,
    // aucune ligne persistee (jamais materialise).
    session.remove(&ih, false).await.expect("remove pending");
    assert!(session.pending_downloads().is_empty());
    let res = add_task.await.expect("task paniquee");
    assert!(
        matches!(res, Err(onionbit_core::CoreError::Cancelled(_))),
        "abandon silencieux attendu : {res:?}"
    );
    let row = session
        .db()
        .with(|c| onionbit_db::downloads::get(c, &onionbit_crypto::hash::from_hex(&ih).unwrap()))
        .expect("get downloads");
    assert!(
        row.is_none(),
        "ligne persistee pour un magnet jamais resolu"
    );
    session.stop().await;
}

/// Redemarrage : chaque download restauré revient sur SA lane
/// (persistee), pas sur la lane par defaut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restauration_par_lane_au_redemarrage() {
    let dir = tempfile::tempdir().unwrap();
    let session = start_anon_session(dir.path()).await;

    let mut hashes = Vec::new();
    for hops in [0u32, 2, 3] {
        let bytes = onionbit_test_support::test_torrent_bytes(
            &format!("resto{hops}.bin"),
            100 + u64::from(hops),
        );
        let dl = session
            .add_torrent_bytes_anon(bytes, true, hops, true, None)
            .await
            .expect("add");
        hashes.push((dl.info_hash_hex(), hops));
    }
    session.stop().await;

    let session = start_anon_session(dir.path()).await;
    session.wait_restored().await;
    for (ih, hops) in &hashes {
        assert_eq!(
            session.owner_engine_hops(ih),
            Some(*hops),
            "download restaure sur la mauvaise lane"
        );
    }
    session.stop().await;
}
