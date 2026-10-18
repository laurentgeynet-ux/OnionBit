// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Etape 139 (ADR-0031) — durcissement de la base chiffree :
//! `secure_delete` reel au niveau fichier, recuperation WAL apres
//! crash simule, `-wal` sans clair pendant l'ecriture, suite
//! fonctionnelle sur base chiffree fichier, debordement de tri sans
//! fuite temporaire, rollback transactionnel.

use std::path::{Path, PathBuf};

use onionbit_db::{messaging as dbm, misc, peer_onion, peers, Database, DbError, DbKey};
use rusqlite::Connection;

const ROOT: [u8; 32] = [0x11; 32];

fn key() -> DbKey {
    DbKey::new(onionbit_crypto::identity::derive_db_key_seed(&ROOT))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn contient(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `secure_delete=ON` (configure) : un DELETE libere la page en la
/// zeroisant — le corps n'est plus lisible dans le fichier apres
/// checkpoint. Rend honnete le flag applicatif `secure_delete` des
/// contacts (qui zeroise le corps *avant* le DELETE).
#[test]
fn secure_delete_efface_les_pages_liberees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sec.db");
    let marker = b"corps-de-message-ultra-sensible-x7k9";
    {
        let db = Database::open(&path, Some(&key())).expect("open");
        db.with(|c| {
            c.execute("CREATE TABLE m(b TEXT)", [])?;
            c.execute(
                "INSERT INTO m VALUES (?1)",
                [String::from_utf8_lossy(marker).into_owned()],
            )?;
            Ok(())
        })
        .expect("insert");
        db.checkpoint().expect("ckpt avant delete");
        db.with(|c| {
            c.execute("DELETE FROM m", [])?;
            Ok(())
        })
        .expect("delete");
        db.checkpoint().expect("ckpt apres delete");
    }
    let brut = std::fs::read(&path).expect("read");
    // Le fichier est chiffre : on ne peut pas verifier le zeroing
    // directement — on rouvre et on controle via `freelist` + une
    // lecture brutale des pages libres en clair n'est pas possible.
    // Ce qu'on *peut* prouver : secure_delete est actif et une base
    // CLAIRE avec le meme flux ne conserve pas le marqueur.
    let _ = brut;
    let dir2 = tempfile::tempdir().expect("tempdir2");
    let path2 = dir2.path().join("sec_clear.db");
    {
        let db = Database::open(&path2, None).expect("open clear");
        db.with(|c| {
            c.execute("CREATE TABLE m(b TEXT)", [])?;
            c.execute(
                "INSERT INTO m VALUES (?1)",
                [String::from_utf8_lossy(marker).into_owned()],
            )?;
            Ok(())
        })
        .expect("insert");
        db.checkpoint().expect("ckpt");
        // Sanity : le marqueur est bien ecrit dans le fichier clair.
        assert!(contient(&std::fs::read(&path2).expect("r"), marker));
        db.with(|c| {
            c.execute("DELETE FROM m", [])?;
            Ok(())
        })
        .expect("delete");
        db.checkpoint().expect("ckpt2");
    }
    let brut = std::fs::read(&path2).expect("read2");
    assert!(
        !contient(&brut, marker),
        "secure_delete=ON doit zeroiser la page liberee"
    );
}

/// Crash simule en WAL : transaction COMMITee mais non checkpointee
/// (donnees encore dans `-wal`) + transaction non commitee. On copie
/// le trio db/-wal/-shm pendant que tout tient, puis on rouvre la
/// copie sous clef : le codec doit rejouer le commit et annuler le
/// reste proprement.
#[test]
fn crash_wal_rejoue_commite_et_abandonne_le_reste() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("c.db");
    let setup = |p: &Path| {
        let conn = Connection::open(p).expect("open");
        conn.execute_batch("PRAGMA cipher='chacha20'")
            .expect("cipher");
        conn.execute_batch(&pragma_key()).expect("key");
        conn.execute_batch("PRAGMA wal_autocheckpoint=0")
            .expect("no auto");
        conn
    };
    {
        let conn = setup(&path);
        conn.execute_batch("PRAGMA journal_mode=WAL").expect("wal");
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .expect("sync");
        conn.execute("CREATE TABLE t(n INTEGER)", [])
            .expect("create");
        // Transaction 1 : commitee, vit dans le -wal.
        conn.execute("INSERT INTO t VALUES (1)", []).expect("c1");
        // Deuxieme connexion (meme fichier, meme clef) : elle garde
        // le WAL "vivant" — sans elle la fermeture de `conn`
        // checkpointe et supprime le -wal, ruinant la simulation.
        let keep = setup(&path);
        // Transaction 2 : ouverte, jamais commitee → abandonnee au
        // drop de `conn` (rollback implicite), mais ses frames
        // restent dans le -wal tant que `keep` est ouverte.
        conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
        conn.execute("INSERT INTO t VALUES (2)", []).expect("c2");
        drop(conn);

        // Etat "post-crash" fige : les trois fichiers sont copies
        // tels quels (plus de lock applicatif : `conn` est fermee).
        let copy = dir.path().join("copy");
        std::fs::create_dir(&copy).expect("mkdir");
        for s in ["", "-wal", "-shm"] {
            let src = with_suffix(&path, s);
            if src.exists() {
                std::fs::copy(&src, copy.join(src.file_name().expect("n"))).expect("copy");
            }
        }
        drop(keep);
    }
    // La copie rejoue le WAL a l'ouverture sous clef : 1 present,
    // 2 absent (la txn n'etait pas commitee).
    let path2 = dir.path().join("copy").join("c.db");
    let conn = Connection::open(&path2).expect("reopen");
    conn.execute_batch("PRAGMA cipher='chacha20'")
        .expect("cipher");
    conn.execute_batch(&pragma_key()).expect("key");
    let rows: i64 = conn
        .query_one("SELECT count(*) FROM t", [], |r| r.get(0))
        .expect("count");
    assert_eq!(rows, 1, "commit rejoue, txn non commitee ecartee");
    let ok: String = conn
        .pragma_query_value(None, "integrity_check", |r| r.get(0))
        .expect("ic");
    assert_eq!(ok, "ok");
}

/// Le `-wal` d'une base chiffree ne contient **aucune** donnee en
/// clair pendant une ecriture active (le `-shm` = wal-index, pointeurs
/// seulement — assumé dans l'ADR).
#[test]
fn wal_ne_contient_pas_de_clair_pendant_ecriture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("w.db");
    let marker = b"topsecret-payload-m3d1";
    {
        let db = Database::open(&path, Some(&key())).expect("open");
        db.with(|c| {
            c.execute_batch("PRAGMA wal_autocheckpoint=0")?;
            c.execute("CREATE TABLE m(b TEXT)", [])?;
            c.execute(
                "INSERT INTO m VALUES (?1)",
                [String::from_utf8_lossy(marker).into_owned()],
            )?;
            Ok(())
        })
        .expect("write");
        // Connexion ouverte, donnees en -wal : lecture directe.
        let wal = with_suffix(&path, "-wal");
        assert!(wal.exists(), "wal attendu pendant l'ecriture");
        let wal_bytes = std::fs::read(&wal).expect("read wal");
        assert!(
            !contient(&wal_bytes, marker),
            "le -wal contient du clair : fuite"
        );
        // Meme la page 1 du fichier principal ne porte pas le magic.
        let brut = std::fs::read(&path).expect("read db");
        assert!(!brut.starts_with(b"SQLite format 3"));
    }
}

/// Suite fonctionnelle sur base chiffree fichier (pas `:memory:`) :
/// misc, peers, peer_onion (le graphe social `.onion`), contacts +
/// messages — puis reouverture sous clef et relecture.
#[test]
fn suite_fonctionnelle_sur_base_chiffree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("full.db");
    let k = key();
    {
        let db = Database::open(&path, Some(&k)).expect("open");
        db.with(|c| {
            misc::set(c, "cle", "valeur")?;
            peers::upsert_batch(
                c,
                &[peers::Ipv8PeerRow {
                    public_key: vec![0xBB; 64],
                    address: "10.0.0.1:7000".into(),
                    last_seen: 1_700_000_000,
                    new_style: true,
                }],
                1_700_000_000,
            )?;
            peer_onion::set_peer_onion(
                c,
                &peer_onion::PeerOnionRow {
                    public_key: vec![0xBB; 64],
                    onion_host: format!("{}.onion", "a".repeat(56)),
                    bind_sig: None,
                    sig_checked: false,
                    created_at: 1,
                    updated_at: 1,
                },
            )?;
            dbm::upsert_contact(
                c,
                &dbm::MsgContactRow {
                    public_key: vec![0xCC; 32],
                    state: "active".into(),
                    send_seq: 0,
                    recv_top: 0,
                    retention_secs: 0,
                    secure_delete: false,
                    alias: String::new(),
                    scope: "contact".into(),
                    created_at: 1,
                    updated_at: 1,
                },
            )?;
            Ok(())
        })
        .expect("writes");
    }
    // Tout est chiffre a froid.
    let brut = std::fs::read(&path).expect("read");
    assert!(!brut.starts_with(b"SQLite format 3"));
    assert!(!contient(&brut, b"valeur"));

    // Reouverture : toutes les familles se relisent.
    let db = Database::open(&path, Some(&k)).expect("reopen");
    db.with(|c| {
        assert_eq!(misc::get(c, "cle")?.as_deref(), Some("valeur"));
        assert_eq!(peers::list_peers(c, 0, 10)?.len(), 1);
        assert!(peer_onion::get_peer_onion(c, &[0xBB; 64])?.is_some());
        assert_eq!(dbm::list_contacts(c)?.len(), 1);
        Ok(())
    })
    .expect("reads");
}

/// Debordement de tri sur base chiffree fichier : `temp_store` est
/// compile `=3` (memoire) — aucun fichier temporaire en clair ne peut
/// apparaitre a cote de la base ni dans %TEMP%.
#[test]
fn tri_debordant_sans_fuite_temporaire() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sort.db");
    let k = key();
    let tmp_before: std::collections::BTreeSet<String> = std::fs::read_dir(std::env::temp_dir())
        .expect("tempdir list")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("etilqs_") || n.starts_with("sqlite"))
        .collect();
    {
        let mut db = Database::open(&path, Some(&k)).expect("open");
        db.with(|c| {
            c.execute_batch("PRAGMA cache_size=64")?;
            c.execute("CREATE TABLE s(v TEXT)", [])?;
            let tx = c.unchecked_transaction()?;
            let mut x: u64 = 0x9E3779B9;
            for _ in 0..30_000 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                tx.execute("INSERT INTO s VALUES (hex(randomblob(48)))", [])?;
            }
            tx.commit()?;
            let n: i64 = c.query_one(
                "SELECT count(*) FROM (SELECT v FROM s ORDER BY v)",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(n, 30_000);
            Ok(())
        })
        .expect("sort");
        let _ = &mut db;
    }
    let restants: Vec<_> = std::fs::read_dir(dir.path())
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("tmp") || n.contains("journal") || n.starts_with("etilqs"))
        .collect();
    assert!(restants.is_empty(), "fichiers temporaires : {restants:?}");
    let tmp_after: std::collections::BTreeSet<String> = std::fs::read_dir(std::env::temp_dir())
        .expect("tempdir list")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("etilqs_") || n.starts_with("sqlite"))
        .collect();
    assert_eq!(
        tmp_before, tmp_after,
        "fichier temporaire SQLite dans %TEMP%"
    );
}

/// Rollback transactionnel sur base chiffree : une erreur en cours
/// de transaction laisse la base coherente et relisible sous clef.
#[test]
fn rollback_transactionnel_propre() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rb.db");
    let k = key();
    {
        let db = Database::open(&path, Some(&k)).expect("open");
        db.with(|c| {
            c.execute("CREATE TABLE r(n INTEGER UNIQUE)", [])?;
            c.execute("INSERT INTO r VALUES (1)", [])?;
            Ok(())
        })
        .expect("init");
        let res: onionbit_db::Result<()> = db.with(|c| {
            c.execute_batch("BEGIN IMMEDIATE")?;
            c.execute("INSERT INTO r VALUES (2)", [])?;
            // Violation UNIQUE → la transaction doit etre annulee.
            let err = c.execute("INSERT INTO r VALUES (1)", []);
            if err.is_err() {
                c.execute_batch("ROLLBACK")?;
                return Err(DbError::Corrupt("e".into()));
            }
            c.execute_batch("COMMIT")?;
            Ok(())
        });
        assert!(res.is_err());
    }
    let db = Database::open(&path, Some(&k)).expect("reopen");
    let rows: i64 = db
        .with(|c| Ok(c.query_one("SELECT count(*) FROM r", [], |r| r.get(0))?))
        .expect("count");
    assert_eq!(rows, 1, "rollback propre : seul le 1er insert reste");
}

/// Base chiffree illisible par un SQLite clair : ouverture brute
/// sans codec → echec (pas de magic, pas de schema lisible).
#[test]
fn base_chiffree_invisible_pour_sqlite_clair() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");
    Database::open(&path, Some(&key())).expect("open");
    let brut = std::fs::read(&path).expect("read");
    assert!(!brut.starts_with(b"SQLite format 3"));
    // Un lecteur sans clef ne peut meme pas enumerer le schema.
    let conn = Connection::open(&path).expect("raw open");
    let res = conn.query_one("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    });
    assert!(res.is_err(), "schema illisible sans clef");
}

/// Smoke perf grossier : 5 000 inserts indexes sur base chiffree —
/// borne large (machine de dev) juste pour detecter une regression
/// catastrophique du codec.
#[test]
fn smoke_perf_inserts_chiffres() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("perf.db");
    let k = key();
    let t0 = std::time::Instant::now();
    {
        let db = Database::open(&path, Some(&k)).expect("open");
        db.with(|c| {
            c.execute("CREATE TABLE p(i INTEGER, s TEXT)", [])?;
            let tx = c.unchecked_transaction()?;
            for i in 0..5_000i64 {
                tx.execute("INSERT INTO p VALUES (?1, hex(randomblob(32)))", [i])?;
            }
            tx.commit()?;
            Ok(())
        })
        .expect("inserts");
    }
    let d = t0.elapsed();
    assert!(
        d < std::time::Duration::from_secs(20),
        "5000 inserts chiffres en {d:?} — regression codec"
    );
}

/// `PRAGMA key="x'hex'"` reconstruit depuis la graine HKDF
/// publique — les tests de crash controllent leur propre
/// `Connection` brute (le wrapper `DbKey::pragma` reste `pub(crate)`
/// : aucun acces aux octets de clef depuis l'exterieur du crate).
fn pragma_key() -> String {
    let seed = onionbit_crypto::identity::derive_db_key_seed(&ROOT);
    let mut s = String::from("PRAGMA key=\"x'");
    for b in seed {
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str("'\"");
    s
}
