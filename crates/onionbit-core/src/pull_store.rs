// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `PullStoreBackend` (trait `onionbit-ipv8::ext`) sur la
//! table `pull_store` SQLite (ADR-0026) — pattern
//! `DbAttestationStore` : `onionbit-ipv8` definit le contrat sans
//! connaitre `onionbit-db`.
//!
//! Le pont est aveugle : il stocke et rend des octets deja chiffres
//! e2e par le deposant. `attest_backfill` pioche dans le meme
//! `DbAttestationStore` que la communaute — une attestation relayee
//! a la meme preuve qu'en direct (signature verifiee a l'ingestion).

use std::sync::Arc;

use onionbit_db::pull_store::{self, PullStoreConfig};
use onionbit_db::Database;
use onionbit_ipv8::ext::{AttestationStore, PullStoreBackend};

use crate::attestation_store::DbAttestationStore;

/// `PullStoreBackend` persistant (roles `bridge`/`gateway`).
pub struct DbPullStore {
    db: Arc<Database>,
    cfg: PullStoreConfig,
    attest: Arc<DbAttestationStore>,
}

impl DbPullStore {
    /// Cree le backend sur la base ouverte + config file + store
    /// d'attestations partage avec la communaute ext.
    pub fn new(db: Arc<Database>, cfg: PullStoreConfig, attest: Arc<DbAttestationStore>) -> Self {
        Self { db, cfg, attest }
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

impl PullStoreBackend for DbPullStore {
    fn mailbox_put(&self, slot: &[u8], blob: &[u8]) -> bool {
        self.db
            .with(|c| {
                pull_store::put(
                    c,
                    &self.cfg,
                    slot,
                    pull_store::KIND_MAILBOX,
                    blob,
                    Self::now(),
                )
            })
            .unwrap_or(false)
    }

    fn mailbox_pull(&self, slot: &[u8]) -> Vec<Vec<u8>> {
        self.db
            .with(|c| pull_store::pull(c, &self.cfg, slot, pull_store::KIND_MAILBOX, Self::now()))
            .unwrap_or_default()
    }

    fn vault_put(&self, slot: &[u8], blob: &[u8]) {
        let _ = self.db.with(|c| {
            pull_store::put(
                c,
                &self.cfg,
                slot,
                pull_store::KIND_VAULT,
                blob,
                Self::now(),
            )
        });
    }

    fn vault_get(&self, slot: &[u8]) -> Option<Vec<u8>> {
        self.db
            .with(|c| pull_store::get(c, &self.cfg, slot, pull_store::KIND_VAULT, Self::now()))
            .ok()
            .and_then(|mut v| if v.len() == 1 { v.pop() } else { None })
    }

    fn attest_backfill(&self, kind: u8, subject: &[u8]) -> Vec<Vec<u8>> {
        // Reutilise le store de la communaute : `by_subject` rend les
        // attestations stockees (deja verifiees a l'ingestion),
        // serialisees par `Attestation::pack`, bornees par
        // `pull_limit` comme le reste du store.
        self.attest
            .by_subject(kind, subject)
            .into_iter()
            .take(self.cfg.pull_limit)
            .map(|a| a.pack())
            .collect()
    }
}
