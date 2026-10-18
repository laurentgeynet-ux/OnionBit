// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Spike ADR-0031 (etape 136) : valide la liaison sqlite3mc derriere
//! `rusqlite` et le comportement du codec ChaCha20 — creation
//! chiffree, reouverture, refus propre sans cle / mauvaise cle, WAL,
//! FTS5, `mmap_size`, absence de spill temporaire en clair et
//! migration `ATTACH ... KEY` depuis une base non chiffree.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Cle brute 256 bits au format SQLCipher (`x'<64 hex>'`) — le codec
/// la prend telle quelle (bypass KDF interne) : notre HKDF est alors
/// le seul niveau de derivation, comme le veut l'ADR-0031.
const KEY: &str = "x'0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'";
const OTHER_KEY: &str = "x'fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210'";

/// Marqueur recherche en clair dans le fichier brut.
const MARKER: &str = "MARQUEUR_SPIKE_ONIONBIT_7f3a9c";

fn pragma_key(key: &str) -> String {
    format!("PRAGMA key=\"{key}\"")
}

fn open_keyed(path: &Path, key: &str) -> Connection {
    let conn = Connection::open(path).expect("open");
    conn.execute_batch("PRAGMA cipher='chacha20'")
        .expect("cipher");
    conn.execute_batch(&pragma_key(key)).expect("key");
    conn
}

fn lit_schema(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_one("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
}

/// Le codec est bien la bibliotheque liee — fail-closed si jamais le
/// linker resout un SQLite systeme vanilla (`PRAGMA cipher` inconnu
/// n'y retourne aucune ligne).
#[test]
fn codec_chacha20_est_la_bibliotheque_liee() {
    let conn = Connection::open_in_memory().expect("memory");
    let cipher: String = conn
        .pragma_query_value(None, "cipher", |r| r.get(0))
        .expect("pragma cipher doit exister sous sqlite3mc");
    assert_eq!(cipher, "chacha20");
    assert_eq!(onionbit_sqlite3mc::SQLITE3MC_VERSION, "2.5.1");
    assert_eq!(onionbit_sqlite3mc::SQLITE_VERSION, "3.53.4");
}

/// La surface de ciphers est volontairement reduite a ChaCha20.
#[test]
fn ciphers_non_chacha20_refuses() {
    for mauvais in ["aes256cbc", "sqlcipher", "rc4", "ascon128"] {
        let conn = Connection::open_in_memory().expect("memory");
        let res = conn.execute_batch(&format!("PRAGMA cipher='{mauvais}'"));
        assert!(
            res.is_err(),
            "cipher {mauvais} devrait etre absent du build"
        );
    }
}

/// Cycle complet : creation chiffree, WAL, reouverture cle, refus
/// propre sans cle et avec mauvaise cle.
#[test]
fn cycle_chiffre_reouverture_et_cles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");

    {
        let conn = open_keyed(&path, KEY);
        conn.execute("CREATE TABLE t(s TEXT)", []).expect("create");
        conn.execute("INSERT INTO t VALUES (?1)", [MARKER])
            .expect("insert");
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(mode.to_lowercase(), "delete");
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("wal");
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(mode.to_lowercase(), "wal");
        // Le sel codec est lisible une fois la base dechiffree.
        let salt: String = conn
            .pragma_query_value(None, "cipher_salt", |r| r.get(0))
            .expect("cipher_salt");
        assert_eq!(salt.len(), 32, "sel codec = 16 octets en hex");
    }

    // Fichier a froid : ni magic SQLite ni contenu lisible.
    let brut = std::fs::read(&path).expect("read db");
    assert!(!brut.starts_with(b"SQLite format 3"));
    assert!(!brut.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));

    // Reouverture avec la cle : donnees intactes.
    {
        let conn = open_keyed(&path, KEY);
        let s: String = conn
            .query_one("SELECT s FROM t", [], |r| r.get(0))
            .expect("select");
        assert_eq!(s, MARKER);
    }

    // Sans cle : erreur (jamais confondu avec une base claire).
    {
        let conn = Connection::open(&path).expect("open");
        assert!(lit_schema(&conn).is_err());
    }

    // Mauvaise cle : erreur propre (HMAC page invalide).
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(&pragma_key(OTHER_KEY)).expect("key");
        assert!(lit_schema(&conn).is_err());
    }
}

/// FTS5 et mmap_size restent utilisables sur base chiffree.
#[test]
fn fts5_et_mmap_sur_base_chiffree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fts.db");
    let conn = open_keyed(&path, KEY);
    conn.pragma_update(None, "mmap_size", 64_i64 * 1024 * 1024)
        .expect("mmap_size accepte (ignore sur pages chiffrees)");
    conn.execute_batch("CREATE VIRTUAL TABLE f USING fts5(body)")
        .expect("fts5 create");
    conn.execute(
        "INSERT INTO f(body) VALUES ('recherche en texte integral')",
        [],
    )
    .expect("fts5 insert");
    let n: i64 = conn
        .query_one("SELECT count(*) FROM f WHERE f MATCH 'integral'", [], |r| {
            r.get(0)
        })
        .expect("fts5 match");
    assert_eq!(n, 1);
}

/// TEMP_STORE=3 (compile) + pragma runtime : un tri volumineux ne
/// doit deposer aucun fichier temporaire en clair, ni a cote de la
/// base ni dans le dossier temporaire du systeme.
#[test]
fn tri_volumineux_sans_spill_en_clair() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sort.db");
    let mut conn = open_keyed(&path, KEY);
    conn.pragma_update(None, "temp_store", "MEMORY")
        .expect("temp_store");
    let ts: i64 = conn
        .pragma_query_value(None, "temp_store", |r| r.get(0))
        .expect("temp_store");
    assert!(ts >= 2, "temp_store doit etre memoire, recu {ts}");
    // Petit cache pour forcer le sorter a deborder vite.
    conn.pragma_update(None, "cache_size", 256_i64)
        .expect("cache_size");

    let db_dir_snapshot = snapshot_dir(dir.path());
    let tmp_dir_snapshot = snapshot_dir(&std::env::temp_dir());

    conn.execute("CREATE TABLE s(v TEXT)", []).expect("create");
    {
        let tx = conn.transaction().expect("tx");
        {
            let mut stmt = tx.prepare("INSERT INTO s VALUES (?1)").expect("prepare");
            for i in 0..30_000u32 {
                // ~180 octets pseudo-aleatoires par ligne : ~5,4 Mio,
                // au-dela du cache de tri — forcerait un fichier temp.
                let v = format!(
                    "{:064x}-{:064x}-{:052x}",
                    i.wrapping_mul(2_654_435_761),
                    i,
                    i
                );
                stmt.execute([v]).expect("insert");
            }
        }
        tx.commit().expect("commit");
    }
    let n: i64 = conn
        .query_one(
            "SELECT count(*) FROM (SELECT v FROM s ORDER BY v)",
            [],
            |r| r.get(0),
        )
        .expect("sort");

    assert_eq!(n, 30_000);
    for nouveau in nouveaux_fichiers(dir.path(), &db_dir_snapshot) {
        let nom = nouveau.file_name().unwrap_or_default().to_string_lossy();
        assert!(
            matches!(
                nom.as_ref(),
                "sort.db-wal" | "sort.db-shm" | "sort.db-journal"
            ),
            "fichier temporaire inattendu pres de la base : {nom}"
        );
    }
    assert!(
        nouveaux_fichiers(&std::env::temp_dir(), &tmp_dir_snapshot).is_empty(),
        "fichier temporaire SQLite dans %TEMP%"
    );
}

fn snapshot_dir(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn nouveaux_fichiers(dir: &Path, avant: &[PathBuf]) -> Vec<PathBuf> {
    snapshot_dir(dir)
        .into_iter()
        .filter(|p| p.is_file() && !avant.contains(p))
        .collect()
}

/// Migration : une base claire existante est attachee a une cible
/// chiffree via `ATTACH ... KEY`, puis copiee — la cible est chiffree
/// (magic absent) et relisible avec la cle.
#[test]
fn migration_base_claire_vers_chiffree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clear = dir.path().join("clear.db");
    let enc = dir.path().join("enc.db");

    {
        let conn = Connection::open(&clear).expect("open clear");
        conn.execute("CREATE TABLE t(s TEXT)", []).expect("create");
        conn.execute("INSERT INTO t VALUES (?1)", [MARKER])
            .expect("insert");
    }
    let brut = std::fs::read(&clear).expect("read clear");
    assert!(brut.starts_with(b"SQLite format 3"), "base source en clair");

    {
        let conn = Connection::open(&clear).expect("reopen clear");
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS enc KEY \"{KEY}\"",
            enc.display()
        ))
        .expect("attach key");
        conn.execute_batch("CREATE TABLE enc.t AS SELECT * FROM main.t")
            .expect("copy");
        conn.execute_batch("DETACH DATABASE enc").expect("detach");
    }

    let brut = std::fs::read(&enc).expect("read enc");
    assert!(!brut.starts_with(b"SQLite format 3"));
    assert!(!brut.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));

    let conn = open_keyed(&enc, KEY);
    let s: String = conn
        .query_one("SELECT s FROM t", [], |r| r.get(0))
        .expect("select");
    assert_eq!(s, MARKER);
}

/// `PRAGMA rekey` permet la rotation en place — principe retenu pour
/// l'etape cle/vault du chantier.
#[test]
fn rekey_rotation_en_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rot.db");
    {
        let conn = open_keyed(&path, KEY);
        conn.execute("CREATE TABLE t(s TEXT)", []).expect("create");
        conn.execute_batch(&format!("PRAGMA rekey=\"{OTHER_KEY}\""))
            .expect("rekey");
    }
    // Ancienne cle refusee, nouvelle cle acceptee.
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(&pragma_key(KEY)).expect("key");
        assert!(lit_schema(&conn).is_err());
    }
    let conn = open_keyed(&path, OTHER_KEY);
    assert!(lit_schema(&conn).is_ok());
}

/// Primitif pour l'etape 138 : `PRAGMA rekey` sur une base **claire**
/// la chiffre en place (semantique sqlite3mc, heritee SQLCipher) —
/// si oui, la migration = copie fichier + rekey (schema, index,
/// triggers, FTS preserves a l'identique, sans rejeu de DDL).
#[test]
fn rekey_chiffre_une_base_claire() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("plain.db");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute("CREATE TABLE t(s TEXT)", []).expect("create");
        conn.execute("INSERT INTO t VALUES ('clair')", [])
            .expect("insert");
    }
    assert!(std::fs::read(&path)
        .expect("read")
        .starts_with(b"SQLite format 3"));
    {
        let conn = Connection::open(&path).expect("reopen");
        conn.execute_batch("PRAGMA cipher='chacha20'")
            .expect("cipher");
        conn.execute_batch(&format!("PRAGMA rekey=\"{KEY}\""))
            .expect("rekey sur clair");
    }
    let brut = std::fs::read(&path).expect("read2");
    assert!(
        !brut.starts_with(b"SQLite format 3"),
        "rekey doit chiffrer le fichier"
    );
    let conn = Connection::open(&path).expect("reopen enc");
    conn.execute_batch(&pragma_key(KEY)).expect("key");
    let s: String = conn
        .query_one("SELECT s FROM t", [], |r| r.get(0))
        .expect("select");
    assert_eq!(s, "clair");
}
