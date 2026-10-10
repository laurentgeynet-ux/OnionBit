// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `PeerStatsStore` sur la base SQLite (`peer_stats`,
//! ADR-0015).
//!
//! `onionbit-tunnel` definit le trait `PeerStatsStore` sans dependre
//! de `onionbit-db` ; `onionbit-db` stocke des lignes brutes sans
//! connaitre `PeerStat`. Ce module est la couture : il convertit les
//! deux representations et delegue a `Database::with` (le flush
//! periodique ne touche que les lignes modifiees — upsert borne par
//! `max_peers`).

use std::sync::Arc;

use onionbit_db::peer_stats::PeerStatRow;
use onionbit_db::Database;
use onionbit_tunnel::peer_stats::{PeerStat, PeerStatsStore};

/// `PeerStatsStore` persistant : table `peer_stats` de `onionbit.db`.
/// Une ecriture en echec est loggee mais n'interrompt jamais la
/// comptabilite — le livre reste fonctionnel en memoire.
pub struct DbPeerStatsStore {
    db: Arc<Database>,
}

impl DbPeerStatsStore {
    /// Cree le store sur la base ouverte.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

/// `u64` borne a `i64::MAX` (SQLite stocke des entiers signes — les
/// compteurs n'atteindront jamais cette valeur).
fn clamp_i64(v: u64) -> i64 {
    v.min(i64::MAX as u64) as i64
}

impl PeerStatsStore for DbPeerStatsStore {
    fn load_peer_stats(&self) -> Vec<(Vec<u8>, PeerStat)> {
        match self.db.with(onionbit_db::peer_stats::load_all) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| {
                    (
                        r.public_key,
                        PeerStat {
                            bytes_served: r.bytes_served.max(0) as u64,
                            bytes_used: r.bytes_used.max(0) as u64,
                            circuits_served: r.circuits_served.max(0) as u64,
                            circuits_used: r.circuits_used.max(0) as u64,
                            first_seen: r.first_seen.max(0) as u64,
                            last_seen: r.last_seen.max(0) as u64,
                        },
                    )
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "chargement de peer_stats impossible — livre vide");
                Vec::new()
            }
        }
    }

    fn upsert_peer_stat(&self, public_key: &[u8], stat: &PeerStat) {
        self.upsert_peer_stats(&[(public_key.to_vec(), stat.clone())]);
    }

    /// Lot d'upserts en UNE transaction : le flush du ledger persiste
    /// des centaines d'entrees — un `.with` par ligne signifiait un
    /// commit/fsync chacun et la connexion verrouillee en continu
    /// (lags executor multi-secondes observes en session).
    fn upsert_peer_stats(&self, batch: &[(Vec<u8>, PeerStat)]) {
        let rows: Vec<PeerStatRow> = batch
            .iter()
            .map(|(public_key, stat)| PeerStatRow {
                public_key: public_key.clone(),
                bytes_served: clamp_i64(stat.bytes_served),
                bytes_used: clamp_i64(stat.bytes_used),
                circuits_served: clamp_i64(stat.circuits_served),
                circuits_used: clamp_i64(stat.circuits_used),
                first_seen: clamp_i64(stat.first_seen),
                last_seen: clamp_i64(stat.last_seen),
            })
            .collect();
        if let Err(e) = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            for r in &rows {
                onionbit_db::peer_stats::upsert(&tx, r)?;
            }
            tx.commit()?;
            Ok(())
        }) {
            tracing::warn!(error = %e, "persistance de peer_stats impossible");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_db_aller_retour_complet() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbPeerStatsStore::new(db);
        assert!(store.load_peer_stats().is_empty());
        store.upsert_peer_stat(
            b"pair-a",
            &PeerStat {
                bytes_served: 123,
                bytes_used: 45,
                circuits_served: 2,
                circuits_used: 1,
                first_seen: 1000,
                last_seen: 2000,
            },
        );
        store.upsert_peer_stat(
            b"pair-b",
            &PeerStat {
                bytes_used: 9,
                ..PeerStat::default()
            },
        );
        let mut got = store.load_peer_stats();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, b"pair-a");
        assert_eq!(got[0].1.bytes_served, 123);
        assert_eq!(got[0].1.deficit(), 78);
        assert_eq!(got[1].0, b"pair-b");
        // Upsert : remplacement.
        store.upsert_peer_stat(
            b"pair-a",
            &PeerStat {
                bytes_served: 200,
                ..PeerStat::default()
            },
        );
        let got = store.load_peer_stats();
        assert_eq!(got.len(), 2);
        assert_eq!(
            got.iter()
                .find(|(pk, _)| pk == b"pair-a")
                .unwrap()
                .1
                .bytes_served,
            200
        );
    }
}
