// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `OpaqueBitVFactory` — enrobage du `BitVFactory` de persistence
//! (ADR-0018, etape 61).
//!
//! Les `.bitv` fastresume sont adresses par `TorrentIdOrHash::Hash`
//! dans librqbit : sans enrobage, le fichier s'appellerait
//! `<infohash>.bitv` en clair dans `state/rqbit/` — un infohash
//! identifiable identifie le contenu via DHT/swarm (fuite de
//! metadonnee). Pour un hash declare **prive** par le moteur, le
//! wrapper substitue `Hash(h)` → `Hash(HMAC20(K_names, "bitv/"‖h))` :
//! le fichier ecrit/lu est `<hmac>.bitv`, indiscernable d'un nom
//! aleatoire.
//!
//! - mapping **par hash** (set memoire alimente par le moteur) —
//!   `K_names` peut ne pas exister (zone verrouillee) sans casser la
//!   zone publique ;
//! - **invite** : un hash prive en session invitee court-circuite en
//!   `NonPersistentBitVFactory` — zero artefact persistant
//!   (coherent avec la purge `.guest/`) ;
//! - **verrouille** (`keys == None`) : `load`/`clear` retombent sur
//!   le nom clair (nettoyage du residu `<ih>.bitv` d'une bascule
//!   public→privee) ; `store_initial_check` refuse — ecrire un
//!   `.bitv` clair d'un prive serait precisement la fuite a eviter ;
//! - `TorrentIdOrHash::Id` : toujours delegue — les prives ne sont
//!   pas inscrits dans `session.json` (patch `update_db`), la
//!   resolution Id→hash n'existe pas pour eux.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use librqbit::api::TorrentIdOrHash;
use librqbit::{BitV, BitVFactory, NonPersistentBitVFactory, BF};
use librqbit_core::Id20;
use onionbit_crypto::obdfile::PrivateStoreKeys;

/// Configuration partagee de l'opacite `.bitv` (ADR-0018).
///
/// Une instance par session moteur : elle produit l'enrobage injecte
/// dans `SessionOptions::bitv_factory_wrapper`, partage le set des
/// infohashes prives avec `PrivateStorageFactory` (qui l'alimente a
/// `create`) et sait supprimer les artefacts `.bitv` d'un telechargement
/// prive a sa suppression (`rqbit` ne les voit pas — les prives sont
/// sautes de `session.json`).
#[derive(Clone)]
pub struct OpaqueBitV {
    /// `None` = zone privee verrouillee ou absente.
    keys: Option<Arc<PrivateStoreKeys>>,
    /// Infohashes prives — partage `Arc` avec
    /// `PrivateStorageFactory::with_private_hashes`, qui l'alimente a
    /// `create` (avant le `bitv.load` de `initializing`).
    private: Arc<Mutex<HashSet<Id20>>>,
    /// Session invitee : les prives ne persistent rien.
    guest: Arc<AtomicBool>,
    /// Dossier rqbit (`state/rqbit`) ou vivent les `.bitv` — requis
    /// pour la suppression directe (`<hmac>.bitv` est invisible pour
    /// `SessionPersistenceStore::delete`, qui ignore les prives).
    persistence_dir: PathBuf,
}

impl std::fmt::Debug for OpaqueBitV {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Les cles et les infohashes prives n'apparaissent jamais.
        f.debug_struct("OpaqueBitV")
            .field("unlocked", &self.keys.is_some())
            .field("guest", &self.guest.load(Ordering::Relaxed))
            .field("persistence_dir", &self.persistence_dir)
            .finish_non_exhaustive()
    }
}

impl OpaqueBitV {
    pub fn new(
        keys: Option<Arc<PrivateStoreKeys>>,
        guest: Arc<AtomicBool>,
        persistence_dir: PathBuf,
    ) -> Self {
        Self {
            keys,
            private: Arc::new(Mutex::new(HashSet::new())),
            guest,
            persistence_dir,
        }
    }

    /// Set partage des infohashes prives — a brancher sur
    /// `PrivateStorageFactory::with_private_hashes`.
    pub fn private_hashes(&self) -> &Arc<Mutex<HashSet<Id20>>> {
        &self.private
    }

    /// `h` est-il declare prive ?
    pub fn is_private(&self, h: &Id20) -> bool {
        self.private
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(h)
    }

    /// Declare `h` prive (en plus du branchement automatique de
    /// `PrivateStorageFactory::create`).
    pub fn mark_private(&self, h: Id20) {
        self.private
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(h);
    }

    /// `<infohash>.bitv` tel que le nomme `JsonSessionPersistenceStore`.
    fn bitv_path(&self, h: &Id20) -> PathBuf {
        self.persistence_dir.join(format!("{h:?}.bitv"))
    }

    /// Suppression d'un telechargement prive (`remove_data`) : rqbit
    /// ne connait pas le `.bitv` opaque (le prive est saute de
    /// `session.json`) — on le supprime directement, ainsi que le
    /// residu `<ih>.bitv` clair d'une eventuelle bascule
    /// public→privee, puis on retire le hash du set.
    pub fn clear_files(&self, h: &Id20) {
        if !self.is_private(h) {
            return;
        }
        let mut paths = vec![self.bitv_path(h)];
        if let Some(k) = &self.keys {
            paths.push(self.bitv_path(&Id20::new(k.bitv_name(&h.0))));
        }
        for p in paths {
            match std::fs::remove_file(&p) {
                Ok(()) => tracing::debug!(file = %p.display(), ".bitv supprime"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(file = %p.display(), error = %e, ".bitv non supprime"),
            }
        }
        self.private
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(h);
    }

    /// Enrobage injecte via `SessionOptions::bitv_factory_wrapper`.
    pub fn wrapper(&self) -> crate::config::BitVWrapperFn {
        let me = self.clone();
        Arc::new(move |inner| me.wrap(inner))
    }

    fn wrap(&self, inner: Arc<dyn BitVFactory>) -> Arc<dyn BitVFactory> {
        Arc::new(OpaqueBitVFactory {
            inner,
            keys: self.keys.clone(),
            private: self.private.clone(),
            guest: self.guest.clone(),
        })
    }
}

pub struct OpaqueBitVFactory {
    inner: Arc<dyn BitVFactory>,
    /// `None` = zone privee verrouillee ou absente.
    keys: Option<Arc<PrivateStoreKeys>>,
    /// Infohashes prives declares — set partage `OpaqueBitV`.
    private: Arc<Mutex<HashSet<Id20>>>,
    /// Session invitee : les prives ne persistent rien.
    guest: Arc<AtomicBool>,
}

impl OpaqueBitVFactory {
    fn is_private(&self, h: &Id20) -> bool {
        self.private
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(h)
    }

    fn guest(&self) -> bool {
        self.guest.load(Ordering::Acquire)
    }

    /// `Hash(h) ∈ prives` → `Hash(HMAC20(K_names,"bitv/"‖h))` ;
    /// `None` quand la cle n'existe pas (verrouille).
    fn map(&self, h: &Id20) -> Option<TorrentIdOrHash> {
        self.keys
            .as_ref()
            .map(|k| TorrentIdOrHash::Hash(Id20::new(k.bitv_name(&h.0))))
    }
}

#[async_trait::async_trait]
impl BitVFactory for OpaqueBitVFactory {
    async fn load(&self, id: TorrentIdOrHash) -> anyhow::Result<Option<Box<dyn BitV>>> {
        match id {
            TorrentIdOrHash::Hash(h) if self.is_private(&h) => {
                if self.guest() {
                    return NonPersistentBitVFactory {}.load(id).await;
                }
                match self.map(&h) {
                    Some(mapped) => self.inner.load(mapped).await,
                    // Verrouille : rien a relire sous ce nom clair.
                    None => self.inner.load(id).await,
                }
            }
            _ => self.inner.load(id).await,
        }
    }

    async fn clear(&self, id: TorrentIdOrHash) -> anyhow::Result<()> {
        match id {
            TorrentIdOrHash::Hash(h) if self.is_private(&h) => {
                if self.guest() {
                    return NonPersistentBitVFactory {}.clear(id).await;
                }
                match self.map(&h) {
                    Some(mapped) => self.inner.clear(mapped).await,
                    // Verrouille : purge du residu en clair (bascule
                    // public→privee).
                    None => self.inner.clear(id).await,
                }
            }
            _ => self.inner.clear(id).await,
        }
    }

    async fn store_initial_check(
        &self,
        id: TorrentIdOrHash,
        b: BF,
    ) -> anyhow::Result<Box<dyn BitV>> {
        match id {
            TorrentIdOrHash::Hash(h) if self.is_private(&h) => {
                if self.guest() {
                    return NonPersistentBitVFactory {}.store_initial_check(id, b).await;
                }
                match self.map(&h) {
                    Some(mapped) => self.inner.store_initial_check(mapped, b).await,
                    None => anyhow::bail!(
                        "refus d'ecrire un .bitv en clair pour un hash prive (zone verrouillee)"
                    ),
                }
            }
            _ => self.inner.store_initial_check(id, b).await,
        }
    }
}
