// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ADR-0018 etape 62 : zone privee liee a l'identite — opacite de la
//! ligne `downloads` (cle HMAC, pas de nom/infohash en clair),
//! manifeste `manifest.obm` chiffre, restauration via OBM, refus
//! `locked`, ephemere invite et rapport d'orphelins.

use onionbit_core::config::StorageArea;
use onionbit_core::identity::{self, IdentityMaterial};
use onionbit_core::{CoreConfig, CoreError, CoreSession, Notifier};

fn ih_bytes(ih: &str) -> Vec<u8> {
    hex::decode(ih).expect("infohash hex")
}

/// Nom de groupe opaque : hex HMAC (>= 32 chars), sans rapport avec
/// le vrai infohash ou le nom de contenu.
fn is_opaque_name(name: &str) -> bool {
    name.len() >= 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Ajout prive : la ligne DB est opaque (cle `HMAC`, pas le vrai
/// infohash, pas de nom/source en clair), les fichiers vivent sous
/// `data/private/temp/<opaque>`, le manifeste chiffre garde la
/// correspondance reelle.
#[tokio::test]
async fn private_add_ligne_opaque_et_manifeste() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("secret.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start");
    assert_eq!(session.private_area_state(), "mounted");

    let dl = session
        .add_torrent_bytes_anon_area(bytes.clone(), true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    // Le download moteur connait le vrai infohash (in-memory seulement).
    assert_eq!(dl.info_hash_hex(), ih);
    assert_eq!(
        session.storage_area_of(&ih_bytes(&ih)),
        StorageArea::Private
    );

    // Cle de ligne opaque : HMAC, jamais le vrai infohash.
    let row_key = session.stored_row_key(&ih_bytes(&ih));
    assert_eq!(row_key.len(), 20);
    assert_ne!(hex::encode(&row_key), ih);

    // Oracle DB : aucune ligne sous le vrai infohash ; la ligne opaque
    // ne porte ni nom ni URI ni octets torrent en clair.
    let conn = rusqlite::Connection::open(cfg.db_path()).unwrap();
    let real_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE infohash = ?1",
            rusqlite::params![ih_bytes(&ih)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(real_rows, 0, "vrai infohash en base publique");
    let (name, uri, tdata, area): (
        Option<String>,
        String,
        Option<Vec<u8>>,
        String,
    ) = conn
        .query_row(
            "SELECT name, source_uri, torrent_data, storage_area FROM downloads WHERE infohash = ?1",
            rusqlite::params![row_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("ligne opaque");
    assert_eq!(area, "private");
    assert!(name.is_none() || name.as_deref().is_some_and(|n| n.is_empty()));
    assert!(uri.is_empty(), "source_uri privee en clair");
    assert!(tdata.is_none() || tdata.as_deref().is_some_and(|t| t.is_empty()));

    // Oracle disque : un seul groupe opaque dans `private/temp`, dont
    // le nom n'est ni le vrai infohash ni le vrai nom de contenu.
    let roots = session.paths().clone();
    let temp = roots.private_temp();
    let groups: Vec<_> = std::fs::read_dir(&temp)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(groups.len(), 1, "groupes temp prives : {groups:?}");
    assert!(
        is_opaque_name(&groups[0]),
        "nom de groupe non opaque : {}",
        groups[0]
    );
    assert_ne!(groups[0], ih);
    // Le manifeste chiffre existe et retrace la correspondance reelle.
    assert!(roots.private_manifest().exists(), "manifest.obm absent");
    let entries = session.private_manifest_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].infohash, ih);
    assert_eq!(entries[0].name.as_deref(), Some("secret.bin"));
    // Aucun fichier `.torrent` en clair n'a fuite vers le backup public.
    let torrents = roots.public_torrents();
    if torrents.exists() {
        let leaked: Vec<_> = std::fs::read_dir(&torrents)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("secret"))
            .collect();
        assert!(
            leaked.is_empty(),
            "backup .torrent prive en clair : {leaked:?}"
        );
    }
    session.stop().await;
}

/// Restauration privee : au redemarrage la ligne opaque est resolue
/// via `manifest.obm` (vrai infohash + octets torrent) — jamais en
/// tentant de re-add la cle opaque comme un infohash.
#[tokio::test]
async fn private_restaure_via_manifeste_obm() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("cycle-priv.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    session.stop().await;

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let restored = session
        .find_download(&ih)
        .unwrap_or_else(|| panic!("download prive {ih} non restaure"));
    assert!(restored.is_paused(), "etat pause prive non restaure");
    assert_eq!(
        session.storage_area_of(&ih_bytes(&ih)),
        StorageArea::Private
    );
    assert_eq!(session.private_manifest_entries().len(), 1);
    session.stop().await;
}

/// Zone verrouillee (`Pending`/`Locked`, identite non resolue) :
/// l'ajout prive est refuse `InvalidState` (l'API le traduit en 409),
/// aucun artefact prive n'est cree.
#[tokio::test]
async fn private_refuse_zone_verrouillee() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("locked.bin", 42);

    // `first_run_gate` sur un state vierge → phase `Pending` : pas
    // d'identite, pas de zone privee.
    let session = CoreSession::start_gated(cfg, Notifier::new(), true)
        .await
        .expect("start gated");
    assert_eq!(session.private_area_state(), "locked");
    let err = session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect_err("ajout prive accepte sans identite");
    assert!(
        matches!(err, CoreError::InvalidState(_)),
        "erreur inattendue : {err:?}"
    );
    session.stop().await;
}

/// Session invitee : la zone est montee sous `temp/.guest/` (ephemere),
/// aucun `manifest.obm` n'est ecrit et tout est purge au `stop`.
#[tokio::test]
async fn private_invite_ephemere() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("guest.bin", 42);

    let session = CoreSession::start_gated(cfg, Notifier::new(), true)
        .await
        .expect("start gated");
    session
        .try_start_identity(Some(IdentityMaterial::guest()))
        .await
        .expect("guest");
    assert!(session.is_guest());
    assert_eq!(session.private_area_state(), "guest");

    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive invite");
    let roots = session.paths().clone();
    let guest_root = roots.private_guest_temp();
    // L'invite produit du contenu ephemere quelque part sous .guest/.
    assert!(guest_root.exists(), "racine invite absente");
    session.stop().await;
    assert!(!guest_root.exists(), "racine invite non purgee au stop");
    assert!(
        !roots.private_manifest().exists(),
        "manifest.obm ecrit pour une session invitee"
    );
}

/// Orphelins : un groupe `.obd` sans entree manifeste est rapporte au
/// montage (jamais supprime silencieusement) et purge a la demande.
#[tokio::test]
async fn private_orphelins_rapportes_et_purges() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("orph.bin", 42);

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    session.stop().await;

    // Orphelin manuel : groupe opaque + fichier `.obd`, sans entree.
    let orphan = dir
        .path()
        .join("data/private/downloads")
        .join("aa".repeat(32));
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("x.obd"), b"orphan").unwrap();

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    let report = session.private_orphan_report();
    assert!(
        report.obd_groups.iter().any(|g| g == &orphan),
        "orphelin non rapporte : {:?}",
        report.obd_groups
    );
    session.purge_private_orphans();
    assert!(!orphan.exists(), "orphelin non purge");
    assert!(session.private_orphan_report().obd_groups.is_empty());
    session.stop().await;
}

/// `locked → unlock` : la graine scellee `OBSK` laisse la ligne privee
/// differee (zone non montee) ; apres `unlock` la zone monte et la
/// restauration rejoue le download via `manifest.obm`.
#[tokio::test]
async fn private_locked_puis_unlock_restaure() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("locked-priv.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    session.stop().await;

    // Active l'at-rest : la graine devient `OBSK`, le prochain boot
    // est `Locked` (zone privee impossible sans mot de passe).
    identity::seal_seed(&cfg.state_dir, b"pw").expect("seal");

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #2 verrouille");
    assert_eq!(session.private_area_state(), "locked");
    // L'ajout prive reste refuse tant que la graine est scellee.
    let err = session
        .add_torrent_bytes_anon_area(
            onionbit_test_support::test_torrent_bytes("x.bin", 7),
            true,
            0,
            false,
            None,
            StorageArea::Private,
        )
        .await
        .expect_err("add prive accepte sous OBSK");
    assert!(matches!(err, CoreError::InvalidState(_)));

    // Unlock : `unlock_seed` rend le materiel, `try_start_identity`
    // monte la zone et rejoue la restauration (route OBM, jamais la
    // cle opaque).
    let material = identity::unlock_seed(&cfg.state_dir, b"pw").expect("unlock");
    session
        .try_start_identity(Some(material))
        .await
        .expect("identite resolue");
    assert_eq!(session.private_area_state(), "mounted");
    session.wait_restored().await;
    let restored = session
        .find_download(&ih)
        .unwrap_or_else(|| panic!("download prive {ih} non restaure apres unlock"));
    assert!(restored.is_paused());
    assert_eq!(
        session.storage_area_of(&ih_bytes(&ih)),
        StorageArea::Private
    );
    session.stop().await;
}

/// Double perte `manifest.obm` + `.bak` : le balayage `scan_ct`
/// reconstruit une entree minimale (magnet `btih`) — les `.obd`
/// restent identifiables avec la seule graine et le groupe n'est pas
/// classe orphelin (reconstruction reussie).
#[tokio::test]
async fn private_manifeste_double_perte_reconstruit() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("rescue.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    let roots = session.paths().clone();
    session.stop().await;

    // Double perte : courant + rotation.
    std::fs::remove_file(roots.private_manifest()).expect("rm manifest.obm");
    let bak = {
        let mut p = roots.private_manifest().into_os_string();
        p.push(".bak");
        std::path::PathBuf::from(p)
    };
    let _ = std::fs::remove_file(&bak);

    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    // La reconstruction scan_ct a recree une entree minimale.
    let entries = session.private_manifest_entries();
    assert_eq!(entries.len(), 1, "reconstruction scan_ct : {entries:?}");
    assert_eq!(entries[0].infohash, ih);
    assert!(
        entries[0].source_uri.contains(&ih),
        "source_uri reconstruite (magnet) attendue : {:?}",
        entries[0].source_uri
    );
    // Le groupe retrouve n'est pas reporte orphelin.
    let report = session.private_orphan_report();
    assert!(
        report.obd_groups.is_empty(),
        "groupe reconstruit classe orphelin : {:?}",
        report.obd_groups
    );
    session.stop().await;
}

/// Suppression privee : ligne opaque, entree manifeste, fichiers
/// `.obd` et fastresume opaque tous retires.
#[tokio::test]
async fn private_suppression_complete() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let bytes = onionbit_test_support::test_torrent_bytes("gone-priv.bin", 42);
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    let ih = meta.info_hash_hex();

    let session = CoreSession::start(cfg.clone(), Notifier::new())
        .await
        .expect("start #1");
    session
        .add_torrent_bytes_anon_area(bytes, true, 0, false, None, StorageArea::Private)
        .await
        .expect("add prive");
    let temp = session.paths().private_temp();
    session.remove(&ih, true).await.expect("remove prive");
    assert!(session.private_manifest_entries().is_empty());
    let rest: Vec<_> = std::fs::read_dir(&temp)
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(rest.is_empty(), "artefacts prives restants : {rest:?}");
    session.stop().await;

    // Redemarrage : rien ne revient.
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("start #2");
    session.wait_restored().await;
    assert!(session.downloads().is_empty(), "prive supprime restaure");
    session.stop().await;
}
