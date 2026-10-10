// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `pull_store` (ADR-0026) : store chiffre borne des ponts —
//! boite aux lettres offline (`MAILBOX`) et coffre replique
//! (`VAULT`). Le pont est aveugle : il ne stocke que des octets deja
//! chiffres de bout en bout par le deposant ; `slot` est
//! `SHA-256(domaine || cle publique)` — la cle elle-meme n'est pas
//! conservee, seul son hash sert d'index.
//!
//! Bornes (toutes dans [`PullStoreConfig`], jamais en dur) : nombre
//! de depots par slot, taille totale du store, TTL, taille de blob.
//! Eviction : expiration d'abord, puis FIFO — le plus recent d'un
//! slot survec a l'ancien.

use rusqlite::{params, Connection};

use crate::Result;

/// `kind` du store : boite aux lettres (multi-depots, `pull`
/// consomme).
pub const KIND_MAILBOX: i64 = 1;
/// `kind` du store : coffre (un seul etat courant par slot, `put`
/// remplace, `get` ne consomme pas).
pub const KIND_VAULT: i64 = 2;

/// Reglages effectifs du store — injectes par le daemon config.
#[derive(Debug, Clone)]
pub struct PullStoreConfig {
    /// Depots `MAILBOX` conserves par slot au maximum (FIFO au-dela).
    pub max_per_slot: i64,
    /// Lignes totales conservees au maximum (tous slots confondus,
    /// FIFO au-dela).
    pub max_total: i64,
    /// Duree de vie d'un depot en secondes (expiration a l'acces et
    /// a la purge periodique).
    pub ttl_secs: i64,
    /// Taille max d'un blob depose (octets) — au-dela le depot est
    /// refuse (`None` retour de `put`).
    pub blob_max: usize,
    /// Blobs rendus par `pull`/`get` au maximum (borne la reponse
    /// ENCAP — un slot sature continue au pull suivant).
    pub pull_limit: usize,
}

impl Default for PullStoreConfig {
    fn default() -> Self {
        Self {
            max_per_slot: 64,
            max_total: 65_536,
            ttl_secs: 7 * 24 * 3600,
            blob_max: 1800,
            pull_limit: 32,
        }
    }
}

/// Depose `blob` dans `slot` sous `kind` ; `now` = secondes Unix.
///
/// - `KIND_MAILBOX` : empile ; FIFO par slot puis global au-dela des
///   bornes, expiration d'abord.
/// - `KIND_VAULT` : remplace l'etat courant du slot (un seul blob).
///
/// Retourne `false` si `blob.len() > cfg.blob_max` (depot refuse —
/// pas d'erreur : l'appelant repond `full`).
pub fn put(
    conn: &Connection,
    cfg: &PullStoreConfig,
    slot: &[u8],
    kind: i64,
    blob: &[u8],
    now: i64,
) -> Result<bool> {
    if blob.len() > cfg.blob_max {
        return Ok(false);
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM pull_store WHERE expires_at <= ?1",
        params![now],
    )?;
    if kind == KIND_VAULT {
        // Coffre : un seul etat courant — le depot remplace.
        tx.execute(
            "DELETE FROM pull_store WHERE slot = ?1 AND kind = ?2",
            params![slot, kind],
        )?;
    } else {
        // Boite aux lettres : FIFO par slot au-dela de la borne.
        let excess: i64 = tx.query_row(
            "SELECT count(*) - ?2 + 1 FROM pull_store WHERE slot = ?1 AND kind = ?3",
            params![slot, cfg.max_per_slot, kind],
            |r| r.get(0),
        )?;
        if excess > 0 {
            tx.execute(
                "DELETE FROM pull_store WHERE seq IN (
                     SELECT seq FROM pull_store WHERE slot = ?1 AND kind = ?2
                     ORDER BY seq LIMIT ?3)",
                params![slot, kind, excess],
            )?;
        }
        // FIFO global : les plus anciens partent en premier.
        let total: i64 = tx.query_row("SELECT count(*) FROM pull_store", [], |r| r.get(0))?;
        let overflow = total + 1 - cfg.max_total;
        if overflow > 0 {
            tx.execute(
                "DELETE FROM pull_store WHERE seq IN (
                     SELECT seq FROM pull_store ORDER BY seq LIMIT ?1)",
                params![overflow],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO pull_store (slot, kind, blob, stored_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![slot, kind, blob, now, now + cfg.ttl_secs],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Retire **et rend** les blobs de `slot`/`kind` (drain de la boite
/// aux lettres — le pull consomme ; `now` filtre les expires et les
/// purge au passage). Ordre FIFO (seq croissant), borne
/// `cfg.pull_limit`.
pub fn pull(
    conn: &Connection,
    cfg: &PullStoreConfig,
    slot: &[u8],
    kind: i64,
    now: i64,
) -> Result<Vec<Vec<u8>>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM pull_store WHERE expires_at <= ?1",
        params![now],
    )?;
    let mut stmt = tx.prepare(
        "SELECT seq, blob FROM pull_store
         WHERE slot = ?1 AND kind = ?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows: Vec<(i64, Vec<u8>)> = stmt
        .query_map(params![slot, kind, cfg.pull_limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for (seq, _) in &rows {
        tx.execute("DELETE FROM pull_store WHERE seq = ?1", params![seq])?;
    }
    tx.commit()?;
    Ok(rows.into_iter().map(|(_, b)| b).collect())
}

/// Lit les blobs de `slot`/`kind` **sans consommer** (coffre —
/// l'etat courant survit au get). Ordre seq croissant, borne
/// `cfg.pull_limit`.
pub fn get(
    conn: &Connection,
    cfg: &PullStoreConfig,
    slot: &[u8],
    kind: i64,
    now: i64,
) -> Result<Vec<Vec<u8>>> {
    conn.execute(
        "DELETE FROM pull_store WHERE expires_at <= ?1",
        params![now],
    )?;
    let mut stmt = conn.prepare(
        "SELECT blob FROM pull_store
         WHERE slot = ?1 AND kind = ?2 ORDER BY seq LIMIT ?3",
    )?;
    let out = stmt
        .query_map(params![slot, kind, cfg.pull_limit as i64], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(out)
}

/// Purge les depots expires — tache periodique du daemon (les acces
/// purgent deja, ceci evite de conserver des blobs dormants jamais
/// demandes).
pub fn purge_expired(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM pull_store WHERE expires_at <= ?1",
        params![now],
    )?)
}

/// Nombre de lignes conservees (metrique / oracles de banc).
pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT count(*) FROM pull_store", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: i64 = KIND_MAILBOX;
    const VT: i64 = KIND_VAULT;

    fn cfg() -> PullStoreConfig {
        PullStoreConfig {
            max_per_slot: 3,
            max_total: 8,
            ttl_secs: 100,
            blob_max: 10,
            pull_limit: 4,
        }
    }

    #[test]
    fn mailbox_put_pull_consomme_fifo() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let slot = [1u8; 32];
            for i in 0..3u8 {
                assert!(put(c, &cfg(), &slot, MB, &[i, i], 10)?);
            }
            let out = pull(c, &cfg(), &slot, MB, 20)?;
            assert_eq!(out, vec![vec![0, 0], vec![1, 1], vec![2, 2]]);
            // Le pull consomme : second pull = vide.
            assert!(pull(c, &cfg(), &slot, MB, 20)?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mailbox_fifo_par_slot_puis_global() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let s1 = [1u8; 32];
            let s2 = [2u8; 32];
            // Borne par slot = 3 : le 4e depot du slot evince le 1er.
            for i in 0..4u8 {
                assert!(put(c, &cfg(), &s1, MB, &[i], 10)?);
            }
            let out = pull(c, &cfg(), &s1, MB, 20)?;
            assert_eq!(out, vec![vec![1], vec![2], vec![3]]);
            // Borne globale = 8 : depots s1 (x3 restants deja
            // consommes -> on repart propre) + s2.
            for i in 0..8u8 {
                assert!(put(c, &cfg(), &s2, MB, &[i, 0], 30)?);
            }
            for i in 0..3u8 {
                assert!(put(c, &cfg(), &s1, MB, &[i, 9], 31)?);
            }
            // 8 (s2) + 3 (s1) = 11 > 8 : les 3 plus anciens partent.
            assert!(count(c)? <= 8);
            // Le plus recent de s1 survit.
            let rest = pull(c, &cfg(), &s1, MB, 40)?;
            assert!(rest.iter().any(|b| b == &vec![2, 9]));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn vault_remplace_et_get_ne_consomme_pas() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let slot = [3u8; 32];
            assert!(put(c, &cfg(), &slot, VT, b"ancien", 10)?);
            assert!(put(c, &cfg(), &slot, VT, b"nouveau", 20)?);
            let g = get(c, &cfg(), &slot, VT, 30)?;
            assert_eq!(g, vec![b"nouveau".to_vec()]);
            // `get` ne consomme pas.
            assert_eq!(get(c, &cfg(), &slot, VT, 30)?, g);
            // `pull` sur un vault consomme quand meme (usage mixte
            // non prevu, mais coherent).
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn expiration_et_borne_blob() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let slot = [4u8; 32];
            assert!(put(c, &cfg(), &slot, MB, &[7; 5], 10)?);
            // Blob trop gros refuse.
            assert!(!put(c, &cfg(), &slot, MB, &[7; 11], 10)?);
            // Expire : ni pull ni get ne le rendent, purge au passage.
            assert!(pull(c, &cfg(), &slot, MB, 200)?.is_empty());
            assert_eq!(count(c)?, 0);
            assert_eq!(purge_expired(c, 300)?, 0);
            Ok(())
        })
        .unwrap();
    }
}
