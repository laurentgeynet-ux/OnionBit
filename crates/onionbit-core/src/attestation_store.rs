// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `AttestationStore` sur la base SQLite (`attestations`,
//! ADR-0015 §6).
//!
//! `onionbit-ipv8::ext` definit le trait `AttestationStore` sans
//! dependre de `onionbit-db` ; `onionbit-db::attestations` stocke des
//! lignes brutes sans connaitre `Attestation`. Ce module est la
//! couture : il convertit les deux representations et delegue a
//! `Database::with`. Les erreurs d'ecriture sont loggees sans
//! interrompre le gossip — une attestation non persistee reste
//! propagee dans la session.

use std::sync::Arc;

use onionbit_db::attestations::AttestationRow;
use onionbit_db::Database;
use onionbit_ipv8::ext::{Attestation, AttestationStore};

/// `AttestationStore` persistant : table `attestations` de
/// `onionbit.db` (dedup `(curateur, kind, sujet)`, latest wins).
pub struct DbAttestationStore {
    db: Arc<Database>,
}

impl DbAttestationStore {
    /// Cree le store sur la base ouverte.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

/// `u64`/`u8` → `i64` SQLite (borne a `i64::MAX` — les valeurs
/// reelles sont tres en deca).
fn clamp_i64(v: u64) -> i64 {
    v.min(i64::MAX as u64) as i64
}

fn to_row(att: &Attestation, added_on: i64) -> AttestationRow {
    AttestationRow {
        curator: att.curator.clone(),
        kind: i64::from(att.kind),
        subject: att.subject.clone(),
        verdict: i64::from(att.verdict),
        ts: clamp_i64(att.ts),
        signature: att.signature.to_vec(),
        added_on,
    }
}

fn from_row(r: AttestationRow) -> Option<Attestation> {
    // La ligne relue doit re-former une attestation valide — les
    // colonnes sont NOT NULL mais on reste defensif (base
    // manipulable hors processus).
    if r.signature.len() != 64 {
        return None;
    }
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&r.signature);
    Some(Attestation {
        version: onionbit_ipv8::ext::attest::ATTEST_VERSION,
        kind: r.kind.clamp(0, u8::MAX as i64) as u8,
        verdict: r.verdict.clamp(0, u8::MAX as i64) as u8,
        ts: r.ts.max(0) as u64,
        subject: r.subject,
        curator: r.curator,
        signature: sig,
    })
}

fn epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

impl AttestationStore for DbAttestationStore {
    fn put(&self, att: &Attestation) -> bool {
        let row = to_row(att, epoch_secs());
        match self
            .db
            .with(move |c| onionbit_db::attestations::upsert(c, &row))
        {
            Ok(changed) => changed,
            Err(e) => {
                tracing::warn!(error = %e, "persistance d'attestation impossible");
                false
            }
        }
    }

    fn get(&self, curator: &[u8], kind: u8, subject: &[u8]) -> Option<Attestation> {
        let curator = curator.to_vec();
        let subject = subject.to_vec();
        match self
            .db
            .with(move |c| onionbit_db::attestations::get(c, &curator, i64::from(kind), &subject))
        {
            Ok(row) => row.and_then(from_row),
            Err(e) => {
                tracing::warn!(error = %e, "lecture d'attestation impossible");
                None
            }
        }
    }

    fn by_subject(&self, kind: u8, subject: &[u8]) -> Vec<Attestation> {
        let subject = subject.to_vec();
        match self
            .db
            .with(move |c| onionbit_db::attestations::by_subject(c, i64::from(kind), &subject))
        {
            Ok(rows) => rows.into_iter().filter_map(from_row).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "lecture d'attestations impossible");
                Vec::new()
            }
        }
    }

    fn latest(&self, limit: usize) -> Vec<Attestation> {
        match self.db.with(move |c| {
            onionbit_db::attestations::latest(c, limit.min(i64::MAX as usize) as i64)
        }) {
            Ok(rows) => rows.into_iter().filter_map(from_row).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "lecture d'attestations recentes impossible");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_ipv8::ext::{attest_kind, attest_verdict};

    #[test]
    fn store_db_aller_retour_latest_wins() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbAttestationStore::new(db);
        let key = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let att = Attestation::sign(
            &key,
            attest_kind::INFOHASH,
            &[0x11; 20],
            attest_verdict::ENDORSE,
            100,
        )
        .unwrap();
        assert!(store.put(&att));
        // Rejeu de la meme attestation : absorbe par la dedup.
        assert!(!store.put(&att));
        let got = store.by_subject(attest_kind::INFOHASH, &[0x11; 20]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ts, 100);
        assert!(got[0].verify());
        // Verdict plus recent : remplace.
        let att2 = Attestation::sign(
            &key,
            attest_kind::INFOHASH,
            &[0x11; 20],
            attest_verdict::FLAG,
            200,
        )
        .unwrap();
        assert!(store.put(&att2));
        let got = store.by_subject(attest_kind::INFOHASH, &[0x11; 20]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].verdict, attest_verdict::FLAG);
        // Verdict plus ancien rejete.
        let att0 = Attestation::sign(
            &key,
            attest_kind::INFOHASH,
            &[0x11; 20],
            attest_verdict::ENDORSE,
            50,
        )
        .unwrap();
        assert!(!store.put(&att0));
        assert_eq!(store.latest(10).len(), 1);
    }
}
