// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Migrations de schema : une base creee a v1 (ou plus ancienne) est
//! migraee vers `SCHEMA_VERSION` a l'ouverture, sans perte de donnees.

use onionbit_db::migrations::{MIGRATIONS, SCHEMA_VERSION};
use onionbit_db::Database;

/// Construit une base figee a la version `version` (migrations
/// `0..version` appliquees manuellement).
fn make_db_at(path: &std::path::Path, version: usize) {
    let conn = rusqlite::Connection::open(path).unwrap();
    for sql in MIGRATIONS.iter().take(version) {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", version as i64)
        .unwrap();
}

#[test]
fn migration_v1_vers_courant_conserve_les_donnees() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onionbit.db");
    make_db_at(&path, 1);

    // Donnee v1 a preserver a travers la migration.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO downloads (infohash, source_uri, output_dir, added_on, paused)
             VALUES (?1, 'magnet:?xt=urn:btih:aa', 'out', 1700000000, 1)",
            rusqlite::params![vec![0x11u8; 20]],
        )
        .unwrap();
    }

    // Ouverture via l'API du crate : migre vers SCHEMA_VERSION.
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        let v: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
        assert_eq!(v, SCHEMA_VERSION);
        // La colonne v2 existe et la ligne v1 est intacte.
        let (ih, hops): (Vec<u8>, i64) =
            c.query_row("SELECT infohash, anon_hops FROM downloads", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        assert_eq!(ih, vec![0x11u8; 20]);
        assert_eq!(hops, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn ouverture_idempotente_ne_rejoue_pas_les_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onionbit.db");
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        c.execute("INSERT INTO misc (name, value) VALUES ('k', 'v')", [])?;
        Ok(())
    })
    .unwrap();
    drop(db);
    // Reouverture : pas de re-migration (les CREATE TABLE
    // echoueraient sur un schema deja present).
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        let v: String = c.query_row("SELECT value FROM misc WHERE name='k'", [], |r| r.get(0))?;
        assert_eq!(v, "v");
        Ok(())
    })
    .unwrap();
}

#[test]
fn base_plus_recente_que_le_crate_refusee() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onionbit.db");
    make_db_at(&path, MIGRATIONS.len());
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
    }
    let res = Database::open(&path);
    assert!(res.is_err(), "base v{} acceptee", SCHEMA_VERSION + 1);
}

/// Migrations v3+v4 : les colonnes de reglages par telechargement sont
/// ajoutees a `downloads` sans perdre les lignes v2, puis lues via
/// la couche modele (round-trip complet des reglages, trackers
/// ajoutes/retires compris).
#[test]
fn migration_v3_reglages_par_download_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onionbit.db");
    make_db_at(&path, 2);
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO downloads (infohash, source_uri, output_dir, added_on, paused, anon_hops)
             VALUES (?1, 'magnet:?xt=urn:btih:bb', 'out', 1700000000, 0, 0)",
            rusqlite::params![vec![0x22u8; 20]],
        )
        .unwrap();
    }

    let db = Database::open(&path).unwrap();
    db.with(|c| {
        // Colonnes v3 presentes avec leurs defauts.
        let row = onionbit_db::downloads::get(c, &[0x22u8; 20])?.unwrap();
        assert_eq!(row.upload_limit, 0);
        assert_eq!(row.queue_position, -1);
        assert!(!row.safe_seeding && !row.auto_managed && !row.user_stopped);
        assert_eq!(row.seeding_ratio, None);
        assert!(row.selected_files.is_none());
        assert!(row.removed_trackers.is_empty());

        // Ecriture des reglages puis relecture (round-trip).
        onionbit_db::downloads::upsert(
            c,
            &onionbit_db::DownloadRow {
                safe_seeding: true,
                user_stopped: true,
                upload_limit: 1024,
                download_limit: 2048,
                seeding_ratio: Some(1.5),
                auto_managed: true,
                queue_position: 3,
                completed_dir: Some("done".into()),
                selected_files: Some(vec![0, 2]),
                file_priorities: Some(vec![4, 7]),
                extra_trackers: vec!["udp://t.local:80".into()],
                removed_trackers: vec!["udp://dead.local:9".into()],
                ..row
            },
        )?;
        let row = onionbit_db::downloads::get(c, &[0x22u8; 20])?.unwrap();
        assert_eq!((row.upload_limit, row.download_limit), (1024, 2048));
        assert_eq!(row.seeding_ratio, Some(1.5));
        assert_eq!(row.queue_position, 3);
        assert!(row.safe_seeding && row.auto_managed && row.user_stopped);
        assert_eq!(row.completed_dir.as_deref(), Some("done"));
        assert_eq!(row.selected_files.as_deref(), Some(&[0, 2][..]));
        assert_eq!(row.file_priorities.as_deref(), Some(&[4, 7][..]));
        assert_eq!(row.extra_trackers, vec!["udp://t.local:80".to_string()]);
        assert_eq!(row.removed_trackers, vec!["udp://dead.local:9".to_string()]);
        Ok(())
    })
    .unwrap();
}
