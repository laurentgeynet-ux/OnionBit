// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Etape 137-138 (ADR-0031) : `Database::open(path, key)` —
//! creation chiffree, reouverture, erreur `Encrypted` typée sur
//! mauvaise clef, migration transparente des bases legacy claires
//! (etape 138 : copie `.enc.tmp` + `rekey` + swap atomique),
//! opt-out `key=None`.

use onionbit_db::{Database, DbError, DbKey};

const ROOT: [u8; 32] = [0x11; 32];
const OTHER_ROOT: [u8; 32] = [0x22; 32];

fn key(root: &[u8; 32]) -> DbKey {
    DbKey::new(onionbit_crypto::identity::derive_db_key_seed(root))
}

/// `<base><suffix>` — meme convention que `db.rs` (suffixe ajoute
/// au nom complet : `onionbit.db` → `onionbit.db.enc.tmp`).
fn with_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    std::path::PathBuf::from(s)
}

/// Creation chiffree via la vraie API : magic absent a froid,
/// relecture des donnees a chaud.
#[test]
fn open_avec_clef_chiffre_et_relit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");
    let k = key(&ROOT);
    {
        let db = Database::open(&path, Some(&k)).expect("open");
        db.with(|c| {
            c.execute("CREATE TABLE t(s TEXT)", [])?;
            c.execute("INSERT INTO t VALUES ('secret')", [])?;
            Ok(())
        })
        .expect("write");
    }
    let brut = std::fs::read(&path).expect("read");
    assert!(!brut.starts_with(b"SQLite format 3"));
    // Reouverture avec la meme clef.
    let db = Database::open(&path, Some(&k)).expect("reopen");
    let s: String = db
        .with(|c| Ok(c.query_one("SELECT s FROM t", [], |r| r.get(0))?))
        .expect("select");
    assert_eq!(s, "secret");
}

/// Base chiffree + clef absente ou etrangere → `DbError::Encrypted`
/// type — jamais `Corrupt` ni succes silencieux.
#[test]
fn base_chiffree_sans_ou_mauvaise_clef_est_encrypted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");
    let k = key(&ROOT);
    Database::open(&path, Some(&k)).expect("open");

    let err = Database::open(&path, None).expect_err("sans clef");
    assert!(matches!(err, DbError::Encrypted), "recu {err:?}");

    let autre = key(&OTHER_ROOT);
    let err = Database::open(&path, Some(&autre)).expect_err("mauvaise clef");
    assert!(matches!(err, DbError::Encrypted), "recu {err:?}");
}

/// Base legacy en clair + clef fournie (ADR-0031 etape 138) :
/// migration transparente — le fichier ressort chiffre, contenu
/// preserve, `.plain.bak` purge apres reouverture verifiee.
#[test]
fn base_legacy_claire_est_migree_a_la_clef() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clear.db");
    {
        let db = Database::open(&path, None).expect("open clear");
        db.with(|c| {
            c.execute("INSERT INTO misc VALUES ('marqueur', 'clair')", [])?;
            Ok(())
        })
        .expect("insert");
    }
    let brut = std::fs::read(&path).expect("read");
    assert!(brut.starts_with(b"SQLite format 3"));

    let k = key(&ROOT);
    let db = Database::open(&path, Some(&k)).expect("reopen migre");
    let v: String = db
        .with(|c| {
            Ok(
                c.query_one("SELECT value FROM misc WHERE name = 'marqueur'", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .expect("probe");
    assert_eq!(v, "clair");
    drop(db);

    let brut = std::fs::read(&path).expect("read2");
    assert!(!brut.starts_with(b"SQLite format 3"), "migree");
    assert!(!with_suffix(&path, ".enc.tmp").exists(), "tmp purge");
    assert!(!with_suffix(&path, ".plain.bak").exists(), "bak purge");
    // Reouverture suivante = chemin chiffre normal.
    let db = Database::open(&path, Some(&k)).expect("reopen2");
    let n: i64 = db
        .with(|c| Ok(c.query_one("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?))
        .expect("probe2");
    assert!(n > 0);
}

/// `.enc.tmp` orphelin (coupure avant le swap) : supprime et
/// migration rejouee — la source n'a jamais ete touchee.
#[test]
fn migration_reprend_apres_orphelin_enc_tmp() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clear.db");
    {
        let db = Database::open(&path, None).expect("open");
        db.with(|c| {
            c.execute("INSERT INTO misc VALUES ('k', 'v')", [])?;
            Ok(())
        })
        .expect("insert");
    }
    // Simule une coupure : residu d'une copie partielle.
    let enc_tmp = with_suffix(&path, ".enc.tmp");
    std::fs::write(&enc_tmp, b"PARTIEL").expect("residu");

    let k = key(&ROOT);
    let db = Database::open(&path, Some(&k)).expect("reprise");
    let v: String = db
        .with(|c| Ok(c.query_one("SELECT value FROM misc WHERE name='k'", [], |r| r.get(0))?))
        .expect("probe");
    assert_eq!(v, "v");
    assert!(!with_suffix(&path, ".enc.tmp").exists());
    assert!(!std::fs::read(&path)
        .expect("read")
        .starts_with(b"SQLite format 3"));
}

/// Crash entre les deux renommages : canonique absente +
/// `.plain.bak` presente → le bak est restaure puis la migration
/// rejoue proprement.
#[test]
fn migration_restaure_bak_si_canonique_absente() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clear.db");
    Database::open(&path, None).expect("open");
    // Etat post-crash : canonique deplacee en bak, copie jamais
    // renommee.
    let bak = with_suffix(&path, ".plain.bak");
    std::fs::rename(&path, &bak).expect("simule crash");
    std::fs::write(with_suffix(&path, ".enc.tmp"), b"PARTIEL").expect("residu");

    let k = key(&ROOT);
    Database::open(&path, Some(&k)).expect("reprise bak");
    assert!(!std::fs::read(&path)
        .expect("read")
        .starts_with(b"SQLite format 3"));
    assert!(!bak.exists());
}

/// WAL non checkpointee : les ecritures en attente dans `-wal`
/// doivent suivre la migration (checkpoint TRUNCATE + VACUUM).
#[test]
fn migration_inclut_wal_non_checkpointe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wal.db");
    {
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch("PRAGMA journal_mode=WAL").expect("wal");
        conn.execute("CREATE TABLE w(n INTEGER)", [])
            .expect("create");
        conn.execute("INSERT INTO w VALUES (7)", [])
            .expect("insert");
        // Pas de checkpoint explicite : le -wal peut encore porter
        // des pages — la migration doit les lire.
    }
    let k = key(&ROOT);
    let db = Database::open(&path, Some(&k)).expect("migre");
    let n: i64 = db
        .with(|c| Ok(c.query_one("SELECT n FROM w", [], |r| r.get(0))?))
        .expect("probe");
    assert_eq!(n, 7);
}

/// Opt-out : `key=None` sur une base claire existante → pas de
/// migration silencieuse, la base reste claire (`db.encrypted=false`).
#[test]
fn base_claire_sans_clef_n_est_jamais_migree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("plain.db");
    Database::open(&path, None).expect("open");
    // Reouverture toujours sans clef : aucune migration forcee.
    Database::open(&path, None).expect("reopen");
    assert!(std::fs::read(&path)
        .expect("read")
        .starts_with(b"SQLite format 3"));
    assert!(!with_suffix(&path, ".plain.bak").exists());
    assert!(!with_suffix(&path, ".enc.tmp").exists());
}

/// `key=None` sur fichier neuf = base claire (opt-out
/// `db.encrypted=false`) — comportement pre-ADR-0031 conserve.
#[test]
fn open_sans_clef_cree_base_claire() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("plain.db");
    Database::open(&path, None).expect("open");
    let brut = std::fs::read(&path).expect("read");
    assert!(brut.starts_with(b"SQLite format 3"));
}

/// La base en memoire reste hors scope chiffrement (session
/// invitee, tests) — aucun pragma codec pose.
#[test]
fn memory_reste_hors_scope() {
    let db = Database::memory().expect("memory");
    let n: i64 = db
        .with(|c| Ok(c.query_one("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?))
        .expect("probe");
    assert!(n > 0);
}

/// Deux `Database::open` sur le meme chemin chiffre dans le meme
/// processus (repli du pattern `guards.rs` : la premiere session
/// n'est pas droppee avant la reouverture — diagnostic du verrou
/// Windows 32 observe sous `--features tor` en etape 132).
#[test]
fn double_open_meme_chef_meme_processus() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onionbit.db");
    let k = onionbit_db::DbKey::new([7u8; 32]);
    let db1 = Database::open(&path, Some(&k)).expect("open #1");
    match Database::open(&path, Some(&k)) {
        Ok(_) => eprintln!("second open OK"),
        Err(e) => panic!("second open ERR: {e:?}"),
    }
    drop(db1);
}
