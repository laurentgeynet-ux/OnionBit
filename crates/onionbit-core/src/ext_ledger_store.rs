// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `LedgerStore` sur la base SQLite (`ext_ledger_links`,
//! ADR-0015 §5 — Phase 9c).
//!
//! `onionbit-ipv8::ext::ledger` definit le trait `LedgerStore` sans
//! dependre de `onionbit-db` ; `onionbit-db::ext_ledger` stocke des
//! lignes brutes sans connaitre `LedgerLink`. Ce module est la
//! couture : conversion des representations + semantique
//! `PutOutcome` (dedup par hash, detection de fork sur les deux
//! positions, classement derriere-tete) portee sur les requetes
//! SQL. Les erreurs d'ecriture sont loggees sans interrompre le
//! protocole — un lien non persiste reste propage dans la session.

use std::sync::Arc;

use onionbit_db::ext_ledger::LedgerLinkRow;
use onionbit_db::Database;
use onionbit_ipv8::ext::ledger::{ChainHead, LedgerLink, LedgerStore, PutOutcome};

/// `LedgerStore` persistant : table `ext_ledger_links` de
/// `onionbit.db` (dedup par hash — les positions forkees sont
/// conservees, jamais ecrasees).
pub struct DbLedgerStore {
    db: Arc<Database>,
}

impl DbLedgerStore {
    /// Cree le store sur la base ouverte.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

/// `u64` → `i64` SQLite (borne a `i64::MAX`).
fn clamp_i64(v: u64) -> i64 {
    v.min(i64::MAX as u64) as i64
}

fn epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn to_row(link: &LedgerLink) -> LedgerLinkRow {
    LedgerLinkRow {
        proposal_id: link.proposal_id().to_vec(),
        hash: link.hash().to_vec(),
        pk_a: link.pk_a.clone(),
        seq_a: clamp_i64(link.seq_a),
        prev_a: link.prev_a.to_vec(),
        pk_b: link.pk_b.clone(),
        seq_b: clamp_i64(link.seq_b),
        prev_b: link.prev_b.to_vec(),
        tx_enc: link.tx_enc.clone(),
        sig_a: link.sig_a.to_vec(),
        sig_b: link.sig_b.to_vec(),
        added_on: epoch_secs(),
    }
}

fn from_row(r: LedgerLinkRow) -> Option<LedgerLink> {
    // La ligne relue doit re-former un lien valide — colonnes NOT
    // NULL mais defensif sur les tailles binaires.
    if r.prev_a.len() != 32 || r.prev_b.len() != 32 || r.sig_a.len() != 64 || r.sig_b.len() != 64 {
        return None;
    }
    let mut prev_a = [0u8; 32];
    prev_a.copy_from_slice(&r.prev_a);
    let mut prev_b = [0u8; 32];
    prev_b.copy_from_slice(&r.prev_b);
    let mut sig_a = [0u8; 64];
    sig_a.copy_from_slice(&r.sig_a);
    let mut sig_b = [0u8; 64];
    sig_b.copy_from_slice(&r.sig_b);
    Some(LedgerLink {
        version: onionbit_ipv8::ext::LEDGER_VERSION,
        pk_a: r.pk_a,
        seq_a: r.seq_a.max(0) as u64,
        prev_a,
        pk_b: r.pk_b,
        seq_b: r.seq_b.max(0) as u64,
        prev_b,
        tx_enc: r.tx_enc,
        sig_a,
        sig_b,
    })
}

impl LedgerStore for DbLedgerStore {
    /// Persiste le lien — la semantique `PutOutcome` est evaluee en
    /// SQL avant l'insert. Identite = `proposal_id` (independante de
    /// `sig_b`) : meme proposition → `Duplicate` sauf upgrade
    /// proposition→sceau ; autre `proposal_id` a la meme position →
    /// `Fork` ; aucune chaine avancee → `BehindHead`.
    fn put(&self, link: &LedgerLink) -> PutOutcome {
        let row = to_row(link);
        let pid = row.proposal_id.clone();
        let sealed = link.is_sealed();
        let pa = (row.pk_a.clone(), row.seq_a);
        let pb = (row.pk_b.clone(), row.seq_b);
        let res = self.db.with(move |c| {
            if let Some(old) = onionbit_db::ext_ledger::by_proposal_id(c, &pid)? {
                let old_sealed = old.sig_b.iter().any(|b| *b != 0);
                if old_sealed || !sealed {
                    return Ok(PutOutcome::Duplicate);
                }
            }
            // Positions occupees : `(pk_a, seq_a)` toujours — la
            // chaine du proposeur avance des l'emission. `(pk_b,
            // seq_b)` seulement une fois `sig_b` posee : une
            // proposition non scellee n'engage pas la position du
            // co-signataire — sinon un REJECT suivi d'une
            // reproposition au meme `seq_b` s'auto-marquerait fork
            // (meme semantique que `InMemoryLedgerStore`, observe au
            // banc `bench_ext_ledger_soak` : forks=2 fantomes).
            let pos_a = onionbit_db::ext_ledger::at_position(c, &pa.0, pa.1)?;
            let mut fork = pos_a.iter().any(|r| r.proposal_id != pid);
            let head_a = onionbit_db::ext_ledger::head_seq(c, &pa.0)?;
            let mut behind = head_a.is_some_and(|h| pa.1 < h);
            if sealed {
                let pos_b = onionbit_db::ext_ledger::at_position_sealed(c, &pb.0, pb.1)?;
                fork = fork || pos_b.iter().any(|r| r.proposal_id != pid);
                let head_b = onionbit_db::ext_ledger::head_seq(c, &pb.0)?;
                behind = behind || head_b.is_some_and(|h| pb.1 < h);
            }
            // Derriere une tete connue = `seq < max` sur l'une des
            // chaines (l'upgrade sceau a la meme position est `New`).
            onionbit_db::ext_ledger::insert(c, &row)?;
            Ok(if fork {
                PutOutcome::Fork
            } else if behind {
                PutOutcome::BehindHead
            } else {
                PutOutcome::New
            })
        });
        match res {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "persistance de lien ledger impossible");
                PutOutcome::Duplicate
            }
        }
    }

    fn link_at(&self, pk: &[u8], seq: u64) -> Option<LedgerLink> {
        let pk = pk.to_vec();
        let seq = clamp_i64(seq);
        match self
            .db
            .with(move |c| onionbit_db::ext_ledger::at_position(c, &pk, seq))
        {
            Ok(rows) => rows.into_iter().find_map(from_row),
            Err(e) => {
                tracing::warn!(error = %e, "lecture de lien ledger impossible");
                None
            }
        }
    }

    fn head(&self, pk: &[u8]) -> Option<ChainHead> {
        let pk = pk.to_vec();
        match self.db.with(move |c| {
            let Some(seq) = onionbit_db::ext_ledger::head_seq(c, &pk)? else {
                return Ok(None);
            };
            let rows = onionbit_db::ext_ledger::at_position(c, &pk, seq)?;
            let hash = rows
                .first()
                .and_then(|r| <[u8; 32]>::try_from(r.hash.as_slice()).ok());
            Ok(hash.map(|h| ChainHead {
                seq: seq as u64,
                hash: h,
            }))
        }) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "lecture de tete ledger impossible");
                None
            }
        }
    }

    fn links_of(&self, pk: &[u8], limit: usize) -> Vec<LedgerLink> {
        let pk = pk.to_vec();
        let limit = limit.min(i64::MAX as usize) as i64;
        match self
            .db
            .with(move |c| onionbit_db::ext_ledger::links_of(c, &pk, limit))
        {
            Ok(rows) => rows.into_iter().filter_map(from_row).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "lecture des liens ledger impossible");
                Vec::new()
            }
        }
    }

    fn latest(&self, limit: usize) -> Vec<LedgerLink> {
        let limit = limit.min(i64::MAX as usize) as i64;
        match self
            .db
            .with(move |c| onionbit_db::ext_ledger::latest(c, limit))
        {
            Ok(rows) => rows.into_iter().filter_map(from_row).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "lecture des liens recents impossible");
                Vec::new()
            }
        }
    }

    fn count(&self) -> usize {
        match self.db.with(onionbit_db::ext_ledger::count) {
            Ok(n) => n.max(0) as usize,
            Err(e) => {
                tracing::warn!(error = %e, "comptage des liens impossible");
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
    use onionbit_ipv8::ext::ledger::{LedgerTx, GENESIS};

    fn sealed_pair() -> (LibNaClSecretKey, LibNaClSecretKey, LedgerLink) {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let tx = LedgerTx::new(2048, 1000);
        let mut link =
            LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        link.cosign(&b).unwrap();
        (a, b, link)
    }

    #[test]
    fn store_db_roundtrip_fork_et_tete() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbLedgerStore::new(db);
        let (a, b, link) = sealed_pair();
        assert_eq!(store.put(&link), PutOutcome::New);
        assert_eq!(store.put(&link), PutOutcome::Duplicate);
        let head = store.head(&link.pk_a).unwrap();
        assert_eq!((head.seq, head.hash), (1, link.hash()));
        // Fork a la meme position.
        let tx2 = LedgerTx::new(9999, 1001);
        let mut lf =
            LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx2).unwrap();
        lf.cosign(&b).unwrap();
        assert_eq!(store.put(&lf), PutOutcome::Fork);
        assert_eq!(store.count(), 2);
        assert_eq!(store.links_of(&link.pk_a, 10).len(), 2);
    }

    /// La proposition stockee non scellee est remplacee par son sceau
    /// (meme `proposal_id`) — jamais de faux fork ni de doublon.
    #[test]
    fn sceau_upgrade_la_proposition() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbLedgerStore::new(db);
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let tx = LedgerTx::new(2048, 1000);
        let prop = LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        assert_eq!(store.put(&prop), PutOutcome::New);
        // Rejeu de la meme proposition : absorbe.
        assert_eq!(store.put(&prop), PutOutcome::Duplicate);
        let mut sealed = prop.clone();
        sealed.cosign(&b).unwrap();
        // Le sceau remplace : pas de fork, pas de doublon, tete = hash scelle.
        assert_eq!(store.put(&sealed), PutOutcome::New);
        assert_eq!(store.count(), 1);
        let head = store.head(&sealed.pk_a).unwrap();
        assert_eq!((head.seq, head.hash), (1, sealed.hash()));
    }

    /// Regression (banc `bench_ext_ledger_soak`) : une proposition
    /// rejetee puis re-proposee au meme `seq_b` n'est PAS un fork —
    /// la position `(pk_b, seq_b)` n'est occupee qu'au sceau. Un
    /// REJECT suivi d'un retry au meme rang produisait `forks=2`
    /// fantomes sur le proposeur (positions a+b du meme evenement).
    #[test]
    fn proposition_rejetee_reproposee_sans_fork() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbLedgerStore::new(db);
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        // Proposition 1 `(seq_a=1, seq_b=1)` — rejetee par B, reste
        // non scellee dans le store d'A.
        let tx1 = LedgerTx::new(8192, 1000);
        let p1 = LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx1).unwrap();
        assert_eq!(store.put(&p1), PutOutcome::New);
        // Resync : re-proposition plafonnee `(seq_a=2, seq_b=1)`.
        let tx2 = LedgerTx::new(1024, 1001);
        let p2 = LedgerLink::propose(&a, &b.public_key(), 2, p1.hash(), 1, GENESIS, &tx2).unwrap();
        assert_eq!(store.put(&p2), PutOutcome::New);
        // Le sceau de p2 n'est pas un fork non plus : la position
        // `(pk_b, 1)` ne compte que des liens scelles.
        let mut sealed2 = p2.clone();
        sealed2.cosign(&b).unwrap();
        assert_eq!(store.put(&sealed2), PutOutcome::New);
        assert_eq!(store.count(), 2);
        let head = store.head(&sealed2.pk_a).unwrap();
        assert_eq!((head.seq, head.hash), (2, sealed2.hash()));
    }
}
