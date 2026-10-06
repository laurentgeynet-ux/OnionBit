// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `ext_ledger_links` (ADR-0015 §5, Phase 9c) : liens
//! bilateraux signes du ledger de contribution — stockes tels quels
//! (auto-portants : les deux cles, les positions dans les deux
//! chaines, le `tx` chiffre pour la paire et les deux signatures sont
//! dans la ligne — un lienrelu est reverifiable sans contexte).
//!
//! `onionbit-db` ne connait pas `onionbit-ipv8` : ce module ne
//! manipule que des lignes brutes — la conversion `LedgerLink` ↔
//! `LedgerLinkRow` et la verification vivent dans
//! `onionbit-core::ext_ledger_store`.
//!
//! Dedup par `proposal_id` (cle primaire : sha256 des champs signes
//! et de `sig_a`, independante de `sig_b`) — le sceau remplace la
//! proposition non scellee en place, un rejeu n'ecrit jamais. La
//! detection de fork repose sur l'index de position : deux lignes de
//! `proposal_id` distincts au meme `(pk, seq)` constituent une
//! equivocation averee ; les deux sont conservees comme preuve
//! (pas de consensus, pas d'ecrasement).

use rusqlite::{params, Connection};

use crate::Result;

const COLS: &str =
    "proposal_id, hash, pk_a, seq_a, prev_a, pk_b, seq_b, prev_b, tx_enc, sig_a, sig_b, added_on";

/// Ligne brute d'`ext_ledger_links`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerLinkRow {
    /// `sha256(champs signes || sig_a)` — identite de la proposition
    /// (cle primaire, independante de `sig_b`).
    pub proposal_id: Vec<u8>,
    /// `sha256` de la forme packee complete — ce que `prev_*`
    /// referencent dans les chaines.
    pub hash: Vec<u8>,
    /// Cle publique du serveur (proposeur, premiere signature).
    pub pk_a: Vec<u8>,
    /// Rang du lien dans la chaine de `pk_a` (genese = 1).
    pub seq_a: i64,
    /// Hash du lien precedent de la chaine de `pk_a` (32 zeros a la
    /// genese).
    pub prev_a: Vec<u8>,
    /// Cle publique du beneficiaire (co-signataire).
    pub pk_b: Vec<u8>,
    /// Rang du lien dans la chaine de `pk_b`.
    pub seq_b: i64,
    /// Hash du lien precedent de la chaine de `pk_b`.
    pub prev_b: Vec<u8>,
    /// `tx` chiffre pour la paire (`pair_seal` X25519).
    pub tx_enc: Vec<u8>,
    /// Signature Ed25519 de `pk_a` (64 octets).
    pub sig_a: Vec<u8>,
    /// Signature Ed25519 de `pk_b` — 64 zeros tant que la proposition
    /// n'est pas co-signee.
    pub sig_b: Vec<u8>,
    /// Date de stockage locale (secondes Unix).
    pub added_on: i64,
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerLinkRow> {
    Ok(LedgerLinkRow {
        proposal_id: r.get("proposal_id")?,
        hash: r.get("hash")?,
        pk_a: r.get("pk_a")?,
        seq_a: r.get("seq_a")?,
        prev_a: r.get("prev_a")?,
        pk_b: r.get("pk_b")?,
        seq_b: r.get("seq_b")?,
        prev_b: r.get("prev_b")?,
        tx_enc: r.get("tx_enc")?,
        sig_a: r.get("sig_a")?,
        sig_b: r.get("sig_b")?,
        added_on: r.get("added_on")?,
    })
}

/// Insere ou remplace un lien par `proposal_id` — le sceau
/// remplace la proposition non scellee de meme identite (upgrade),
/// un rejeu du meme contenu est un no-op cote appelant.
pub fn insert(conn: &Connection, row: &LedgerLinkRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR REPLACE INTO ext_ledger_links
         (proposal_id, hash, pk_a, seq_a, prev_a, pk_b, seq_b, prev_b, tx_enc, sig_a, sig_b, added_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            row.proposal_id,
            row.hash,
            row.pk_a,
            row.seq_a,
            row.prev_a,
            row.pk_b,
            row.seq_b,
            row.prev_b,
            row.tx_enc,
            row.sig_a,
            row.sig_b,
            row.added_on,
        ],
    )?;
    Ok(n > 0)
}

/// Lien stocke par `proposal_id` exact (dedup/upgrade).
pub fn by_proposal_id(conn: &Connection, pid: &[u8]) -> Result<Option<LedgerLinkRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ext_ledger_links WHERE proposal_id = ?1"
    ))?;
    let mut rows = stmt.query_map(params![pid], from_row)?;
    Ok(rows.next().transpose()?)
}

/// Tous les liens occupant la position `(pk, seq)` dans une chaine —
/// la position peut etre cote `a` ou `b` du lien (un lien vit
/// dans les deux chaines) ; plusieurs lignes = fork avere.
pub fn at_position(conn: &Connection, pk: &[u8], seq: i64) -> Result<Vec<LedgerLinkRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ext_ledger_links
         WHERE (pk_a = ?1 AND seq_a = ?2) OR (pk_b = ?1 AND seq_b = ?2)
         ORDER BY added_on"
    ))?;
    let rows = stmt.query_map(params![pk, seq], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Liens occupant la position `(pk, seq)` vus du point de vue d'un
/// lien scelle entrant : cote `pk_b`, seules les lignes **scellees**
/// comptent — une proposition non scellee n'engage pas la position
/// du co-signataire (rejet + re-proposition au meme rang ≠ fork,
/// semantique `InMemoryLedgerStore`).
pub fn at_position_sealed(conn: &Connection, pk: &[u8], seq: i64) -> Result<Vec<LedgerLinkRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ext_ledger_links
         WHERE (pk_a = ?1 AND seq_a = ?2)
            OR (pk_b = ?1 AND seq_b = ?2 AND sig_b <> zeroblob(64))
         ORDER BY added_on"
    ))?;
    let rows = stmt.query_map(params![pk, seq], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Tete de la chaine de `pk` : le plus grand rang connu ou `pk`
/// est partie, quel que soit le role.
pub fn head_seq(conn: &Connection, pk: &[u8]) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(seq) FROM (
             SELECT seq_a AS seq FROM ext_ledger_links WHERE pk_a = ?1
             UNION ALL
             SELECT seq_b AS seq FROM ext_ledger_links WHERE pk_b = ?1)",
        params![pk],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

/// Liens dont `pk` est partie (roles confondus), `limit` max.
pub fn links_of(conn: &Connection, pk: &[u8], limit: i64) -> Result<Vec<LedgerLinkRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ext_ledger_links
         WHERE pk_a = ?1 OR pk_b = ?1
         ORDER BY added_on DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![pk, limit], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Les `limit` liens les plus recemment stockes.
pub fn latest(conn: &Connection, limit: i64) -> Result<Vec<LedgerLinkRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ext_ledger_links ORDER BY added_on DESC, rowid DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Nombre de liens stockes.
pub fn count(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM ext_ledger_links", [], |r| r.get(0))
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u8, hash: u8, pk_a: u8, seq_a: i64, pk_b: u8, seq_b: i64) -> LedgerLinkRow {
        LedgerLinkRow {
            proposal_id: vec![pid; 32],
            hash: vec![hash; 32],
            pk_a: vec![pk_a; 42],
            seq_a,
            prev_a: vec![0; 32],
            pk_b: vec![pk_b; 42],
            seq_b,
            prev_b: vec![0; 32],
            tx_enc: vec![7; 40],
            sig_a: vec![1; 64],
            sig_b: vec![2; 64],
            added_on: 5000,
        }
    }

    #[test]
    fn insert_dedup_et_positions() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let r = row(1, 1, 10, 1, 20, 1);
            assert!(insert(c, &r)?);
            // Meme `proposal_id` : remplace en place (le sceau
            // upgrade la proposition — le compte reste 1, pas de
            // doublon, pas de faux fork a la position).
            assert!(insert(c, &r)?);
            assert_eq!(count(c)?, 1);
            let relu = by_proposal_id(c, &[1u8; 32])?.unwrap();
            assert_eq!(relu.pk_a, vec![10u8; 42]);
            // Position (10, 1) et (20, 1) renvoient le lien.
            assert_eq!(at_position(c, &[10u8; 42], 1)?.len(), 1);
            assert_eq!(at_position(c, &[20u8; 42], 1)?.len(), 1);
            assert_eq!(head_seq(c, &[10u8; 42])?, Some(1));
            // Fork : autre hash a la meme position (10, 1).
            let fork = row(2, 2, 10, 1, 20, 5);
            assert!(insert(c, &fork)?);
            assert_eq!(at_position(c, &[10u8; 42], 1)?.len(), 2);
            Ok(())
        })
        .unwrap();
    }
}
