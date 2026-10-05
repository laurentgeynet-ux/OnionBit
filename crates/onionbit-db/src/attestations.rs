// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `attestations` (ADR-0015 §6) : verdicts de curation signes
//! Ed25519, stockes tels quels (auto-portants : `curator` +
//! `signature` dans la ligne — une attestation relue est reverifiable
//! sans contexte).
//!
//! `onionbit-db` ne connait pas `onionbit-ipv8` : ce module ne
//! manipule que des lignes brutes — la conversion `Attestation` ↔
//! `AttestationRow` et la reverification vivent dans
//! `onionbit-core::attestation_store`.
//!
//! Dedup `(curator, kind, subject)` avec *latest wins* : un upsert
//! n'ecrase que si `ts` est strictement plus recent — un vieux
//! verdict rejoue ne re-ecrit jamais un verdict plus recent.

use rusqlite::{params, Connection};

use crate::Result;

const COLS: &str = "curator, kind, subject, verdict, ts, signature, added_on";

/// Ligne brute d'`attestations` (horodatages en secondes Unix).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttestationRow {
    /// Cle publique LibNaCl binaire du curateur signataire.
    pub curator: Vec<u8>,
    /// Nature du sujet (1 = info-hash 20 B, 2 = `LibNaClPK` canal).
    pub kind: i64,
    /// Verdict (1 = endorse, 2 = flag).
    pub verdict: i64,
    /// Horodatage createur de l'attestation.
    pub ts: i64,
    /// Sujet vise.
    pub subject: Vec<u8>,
    /// Signature Ed25519 (64 octets).
    pub signature: Vec<u8>,
    /// Date de stockage locale.
    pub added_on: i64,
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AttestationRow> {
    Ok(AttestationRow {
        curator: r.get("curator")?,
        kind: r.get("kind")?,
        subject: r.get("subject")?,
        verdict: r.get("verdict")?,
        ts: r.get("ts")?,
        signature: r.get("signature")?,
        added_on: r.get("added_on")?,
    })
}

/// Insere ou remplace une attestation — *latest wins* : le
/// remplacement n'a lieu que si `row.ts` est strictement plus recent
/// que le `ts` stocke (un vieux verdict rejoue est absorbe par la
/// dedup sans re-ecriture). Retourne `true` si la ligne a change.
pub fn upsert(conn: &Connection, row: &AttestationRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT INTO attestations
         (curator, kind, subject, verdict, ts, signature, added_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(curator, kind, subject) DO UPDATE SET
            verdict   = excluded.verdict,
            ts        = excluded.ts,
            signature = excluded.signature,
            added_on  = excluded.added_on
         WHERE excluded.ts > attestations.ts",
        params![
            row.curator,
            row.kind,
            row.subject,
            row.verdict,
            row.ts,
            row.signature,
            row.added_on,
        ],
    )?;
    Ok(n > 0)
}

/// Attestation stockee pour la cle exacte
/// `(curator, kind, subject)` — acces cle primaire utilise comme
/// lookup de dedup **avant** verification cryptographique cote
/// protocole (un rejeu est absorbe sans Ed25519).
pub fn get(
    conn: &Connection,
    curator: &[u8],
    kind: i64,
    subject: &[u8],
) -> Result<Option<AttestationRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM attestations
         WHERE curator = ?1 AND kind = ?2 AND subject = ?3"
    ))?;
    let mut rows = stmt.query_map(params![curator, kind, subject], from_row)?;
    Ok(rows.next().transpose()?)
}

/// Attestations stockees pour un sujet (une par curateur au plus).
pub fn by_subject(conn: &Connection, kind: i64, subject: &[u8]) -> Result<Vec<AttestationRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM attestations WHERE kind = ?1 AND subject = ?2"
    ))?;
    let rows = stmt.query_map(params![kind, subject], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Les `limit` attestations les plus recentes (`ts` decroissant).
pub fn latest(conn: &Connection, limit: i64) -> Result<Vec<AttestationRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM attestations ORDER BY ts DESC, rowid DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit], from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(curator: u8, ts: i64, verdict: i64) -> AttestationRow {
        AttestationRow {
            curator: vec![curator; 42],
            kind: 1,
            subject: vec![0x5a; 20],
            verdict,
            ts,
            signature: vec![9; 64],
            added_on: 5000,
        }
    }

    #[test]
    fn upsert_latest_wins_dedup() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            assert!(upsert(c, &row(1, 100, 1))?);
            // Meme cle, ts plus ancien : absorbe, pas de re-ecriture.
            assert!(!upsert(c, &row(1, 50, 2))?);
            // ts strictement plus recent : remplace.
            assert!(upsert(c, &row(1, 200, 2))?);
            let got = by_subject(c, 1, &[0x5a; 20])?;
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].verdict, 2);
            assert_eq!(got[0].ts, 200);
            // Second curateur, meme sujet : ligne separee.
            assert!(upsert(c, &row(2, 150, 1))?);
            assert_eq!(by_subject(c, 1, &[0x5a; 20])?.len(), 2);
            assert_eq!(latest(c, 1)?[0].ts, 200);
            // Lookup cle primaire : le dernier verdict du curateur 1.
            let g = get(c, &[1u8; 42], 1, &[0x5a; 20])?.expect("row");
            assert_eq!(g.ts, 200);
            assert_eq!(g.verdict, 2);
            // Cle absente : `None`, pas d'erreur.
            assert!(get(c, &[9u8; 42], 1, &[0x5a; 20])?.is_none());
            Ok(())
        })
        .unwrap();
    }
}
