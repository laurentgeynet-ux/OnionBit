// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Etape 137 (ADR-0031) : `Database::open(path, key)` — creation
//! chiffree, reouverture, erreur `Encrypted` typée sur mauvaise
//! clef, base legacy claire ouverte sans clef (migration differee
//! a l'etape 138), opt-out `key=None`.

use onionbit_db::{Database, DbError, DbKey};

const ROOT: [u8; 32] = [0x11; 32];
const OTHER_ROOT: [u8; 32] = [0x22; 32];

fn key(root: &[u8; 32]) -> DbKey {
    DbKey::new(onionbit_crypto::identity::derive_db_key_seed(root))
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

/// Base legacy en clair + clef fournie : detection par magic →
/// ouverture claire (la migration propre est l'etape 138) — le
/// fichier n'est pas casse par un `PRAGMA key` premature.
#[test]
fn base_legacy_claire_s_ouvre_meme_avec_clef() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clear.db");
    Database::open(&path, None).expect("open clear");
    let brut = std::fs::read(&path).expect("read");
    assert!(brut.starts_with(b"SQLite format 3"));

    let k = key(&ROOT);
    let db = Database::open(&path, Some(&k)).expect("reopen");
    let n: i64 = db
        .with(|c| Ok(c.query_one("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?))
        .expect("probe");
    assert!(n > 0, "schema legacy lisible");
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
