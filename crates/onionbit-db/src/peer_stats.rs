// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `peer_stats` (ADR-0015) : comptabilite locale des echanges
//! de tunnels par pair.
//!
//! `onionbit-db` ne connait pas `onionbit-tunnel` (sens des
//! dependances) : ce module ne manipule que des lignes brutes —
//! l'adaptation vers `PeerStat`/`PeerStatsStore` vit dans
//! `onionbit-core::peer_stats_store`. L'identite d'un compte est la
//! cle publique du pair (un changement d'adresse conserve le compte).

use rusqlite::{params, Connection};

use crate::Result;

/// Ligne brute de `peer_stats` (horodatages en secondes Unix).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStatRow {
    /// Cle publique binaire du pair (`public_key_bin`).
    pub public_key: Vec<u8>,
    /// Octets transportes pour ses circuits (nous = premier saut ou
    /// relais de son `create`).
    pub bytes_served: i64,
    /// Octets transportes par lui sur nos circuits (tout saut
    /// verifie).
    pub bytes_used: i64,
    /// Circuits qu'il nous a demandes (`create` admis).
    pub circuits_served: i64,
    /// Circuits ou il figure comme saut verifie.
    pub circuits_used: i64,
    /// Premiere observation.
    pub first_seen: i64,
    /// Derniere activite comptabilisee.
    pub last_seen: i64,
}

/// Charge toute la table (un appel au demarrage du livre).
pub fn load_all(conn: &Connection) -> Result<Vec<PeerStatRow>> {
    let mut stmt = conn.prepare(
        "SELECT public_key, bytes_served, bytes_used, circuits_served,
                circuits_used, first_seen, last_seen
         FROM peer_stats",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PeerStatRow {
            public_key: r.get(0)?,
            bytes_served: r.get(1)?,
            bytes_used: r.get(2)?,
            circuits_served: r.get(3)?,
            circuits_used: r.get(4)?,
            first_seen: r.get(5)?,
            last_seen: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Insere ou remplace une entree (flush des lignes modifiees depuis
/// le dernier appel — `INSERT OR REPLACE`, compteurs cumulatifs
/// deja resolus en memoire).
pub fn upsert(conn: &Connection, row: &PeerStatRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO peer_stats
         (public_key, bytes_served, bytes_used, circuits_served,
          circuits_used, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.public_key,
            row.bytes_served,
            row.bytes_used,
            row.circuits_served,
            row.circuits_used,
            row.first_seen,
            row.last_seen,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_stats_aller_retour_upsert() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            assert!(load_all(c)?.is_empty());
            let row = PeerStatRow {
                public_key: vec![7; 64],
                bytes_served: 100,
                bytes_used: 40,
                circuits_served: 3,
                circuits_used: 2,
                first_seen: 1000,
                last_seen: 2000,
            };
            upsert(c, &row)?;
            assert_eq!(load_all(c)?, vec![row.clone()]);
            // Upsert : remplacement, pas de doublon.
            let mut maj = row.clone();
            maj.bytes_served = 150;
            upsert(c, &maj)?;
            let got = load_all(c)?;
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].bytes_served, 150);
            Ok(())
        })
        .unwrap();
    }
}
