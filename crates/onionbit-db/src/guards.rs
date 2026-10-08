// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `guards` (ADR-0010) : persistance des premiers sauts de la
//! `TunnelCommunity`.
//!
//! `onionbit-db` ne connait pas `onionbit-tunnel` (sens des
//! dependances) : ce module ne manipule que des lignes brutes —
//! l'adaptation vers `GuardRecord`/`GuardStore` vit dans
//! `onionbit-core::guard_store`. L'identite d'un guard est sa cle
//! publique ; `position` conserve l'ordre semantique du set (actifs
//! d'abord, reserve ensuite) pour `order_first_hops`.

use rusqlite::{params, Connection};

use crate::Result;

/// Ligne brute de `guards` (horodatages en secondes Unix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRow {
    /// Cle publique binaire du pair (`public_key_bin`).
    pub public_key: Vec<u8>,
    /// Derniere adresse ayant repondu, `"ip:port"` numerique
    /// (`""` = inconnue — un `create` vers elle expirera en timeout).
    pub address: String,
    /// Date d'adoption (rotation a `GuardsConfig::lifetime`).
    pub adopted_at: i64,
    /// Derniere preuve de vie (`created` reussi).
    pub last_seen: i64,
    /// Echecs de handshake `create` consecutifs.
    pub failures: i64,
    /// `true` = reserve, `false` = actif.
    pub reserve: bool,
    /// Rang dans le set (actifs puis reserve) — restaure l'ordre au
    /// chargement, independant de la cle primaire.
    pub position: i64,
}

/// Charge le set complet, trie par `position` (actifs puis reserve).
pub fn load(conn: &Connection) -> Result<Vec<GuardRow>> {
    let mut stmt = conn.prepare(
        "SELECT public_key, address, adopted_at, last_seen, failures, reserve, position
         FROM guards ORDER BY position",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(GuardRow {
            public_key: r.get(0)?,
            address: r.get(1)?,
            adopted_at: r.get(2)?,
            last_seen: r.get(3)?,
            failures: r.get(4)?,
            reserve: r.get::<_, i64>(5)? != 0,
            position: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Remplace le set complet en une transaction (<= 5 lignes — cout
/// trivial, meme pattern snapshot que `tunnel_pex`).
pub fn replace_all(conn: &Connection, rows: &[GuardRow]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM guards", [])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO guards (public_key, address, adopted_at, last_seen, failures, reserve, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (i, g) in rows.iter().enumerate() {
            stmt.execute(params![
                g.public_key,
                g.address,
                g.adopted_at,
                g.last_seen,
                g.failures,
                g.reserve as i64,
                i as i64,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_snapshot_aller_retour() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            assert!(load(c)?.is_empty());
            let rows = vec![
                GuardRow {
                    public_key: vec![1; 64],
                    address: "1.2.3.4:8090".into(),
                    adopted_at: 100,
                    last_seen: 200,
                    failures: 0,
                    reserve: false,
                    position: 0,
                },
                GuardRow {
                    public_key: vec![2; 64],
                    address: String::new(),
                    adopted_at: 110,
                    last_seen: 210,
                    failures: 2,
                    reserve: true,
                    position: 1,
                },
            ];
            replace_all(c, &rows)?;
            let got = load(c)?;
            // `position` est reecrit par replace_all (rang = index).
            assert_eq!(got, rows);
            // Snapshot suivant : suppression + ordre inverse conserve.
            let mut rev = rows.clone();
            rev.reverse();
            for (i, r) in rev.iter_mut().enumerate() {
                r.position = i as i64;
            }
            replace_all(c, &rev)?;
            let got2 = load(c)?;
            assert_eq!(got2, rev);
            assert_eq!(got2.len(), 2);
            Ok(())
        })
        .unwrap();
    }
}
