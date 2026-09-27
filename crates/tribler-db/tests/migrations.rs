//! Migrations de schema : une base creee a v1 (ou plus ancienne) est
//! migraee vers `SCHEMA_VERSION` a l'ouverture, sans perte de donnees.

use tribler_db::migrations::{MIGRATIONS, SCHEMA_VERSION};
use tribler_db::Database;

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
    let path = dir.path().join("tribler.db");
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
    let path = dir.path().join("tribler.db");
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
    let path = dir.path().join("tribler.db");
    make_db_at(&path, MIGRATIONS.len());
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
    }
    let res = Database::open(&path);
    assert!(res.is_err(), "base v{} acceptee", SCHEMA_VERSION + 1);
}
