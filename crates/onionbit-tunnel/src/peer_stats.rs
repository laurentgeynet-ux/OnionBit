// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Comptabilite locale par pair (ADR-0015, Phase 9a).
//!
//! Mesure **pure** — aucun octet supplementaire sur le fil :
//! l'attribution exploite ce que le protocole revele deja.
//!
//! - `bytes_served` : octets transportes pour les circuits joints par
//!   un `create` direct. Le `requester` du `create` **est**
//!   l'initiateur — seul le premier saut connait son identite ; un
//!   saut intermediaire ne voit que ses voisins et ne peut rien
//!   imputer.
//! - `bytes_used` : octets transportes par chaque saut **verifie** de
//!   nos propres circuits. L'initiateur connait toute la route et
//!   chaque saut a reellement porte le volume
//!   (`bytes_up + bytes_down`).
//!
//! La gate d'admission (`enforce`) ne s'applique que sous pression
//! (`joined >= soft_cap`) : un `create` n'est admis que si la dette
//! du pair `served - used <= max_deficit_bytes`. Un pair inconnu a
//! une dette nulle → toujours admis ; le credit de demarrage *est*
//! `max_deficit`. Un pair rembourse en servant **nos** circuits —
//! c'est nous qui choisissons nos sauts, aucun deadlock de
//! remboursement n'est possible.
//!
//! Extension Rust sans equivalent pyipv8 : le protocole filaire est
//! inchange, seule la *decision locale* d'admission des `create`
//! change quand `enforce` est actif.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Reglages de la comptabilite (aucune valeur en dur ailleurs).
#[derive(Debug, Clone)]
pub struct LedgerConfig {
    /// Collecte des compteurs (defaut `true`). `false` = aucune
    /// mesure, aucune gate — comportement pyipv8 exact.
    pub enabled: bool,
    /// Gate d'admission des `create` sous pression (defaut `false` —
    /// mesure experimentale, promotion apres validation terrain comme
    /// les guards ADR-0010).
    pub enforce: bool,
    /// Seuil de `joined` (relays + sorties) a partir duquel la gate
    /// s'applique. En dessous, tout est admis (pyipv8 exact — pas de
    /// gel du bootstrap). Doit etre `< max_joined_circuits`.
    pub soft_cap: usize,
    /// Dette maximale toleree par pair (`served - used`) quand la
    /// gate est active, en octets. Double role : credit de demarrage
    /// accorde a tout inconnu et plafond de deficit.
    pub max_deficit_bytes: u64,
    /// Cadence du comptage par deltas + flush persistant.
    pub tick: Duration,
    /// Borne de la table (croissance controlee face a un flot de cles
    /// fraiches — au-dela, les nouvelles cles ne sont plus suivies).
    pub max_peers: usize,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enforce: false,
            soft_cap: 80,
            max_deficit_bytes: 256 * 1024 * 1024,
            tick: Duration::from_secs(30),
            max_peers: 8192,
        }
    }
}

/// Compteurs d'un pair, indexe par sa cle publique LibNaCl binaire.
/// Horodatages en secondes Unix (`pex::epoch_secs`).
#[derive(Debug, Clone, Default)]
pub struct PeerStat {
    /// Octets transportes pour ses circuits (nous = premier saut ou
    /// relais de son `create`).
    pub bytes_served: u64,
    /// Octets transportes par lui sur nos circuits (tout saut
    /// verifie).
    pub bytes_used: u64,
    /// Circuits qu'il nous a demandes (`create` admis).
    pub circuits_served: u64,
    /// Circuits ou il figure comme saut verifie.
    pub circuits_used: u64,
    /// Premiere observation.
    pub first_seen: u64,
    /// Derniere activite comptabilisee.
    pub last_seen: u64,
}

impl PeerStat {
    /// Dette du pair envers nous : `served - used`, saturee a 0.
    /// 0 = il nous doit rien ou nous sommes crediteurs.
    pub fn deficit(&self) -> u64 {
        self.bytes_served.saturating_sub(self.bytes_used)
    }
}

/// Persistance injectable — `onionbit-tunnel` ne depend pas de
/// `onionbit-db` (sens des dependances inverse, meme couture que
/// `GuardStore`). Implementee par la table `peer_stats` cote
/// `onionbit-db`, injectee par `core`/`daemon`.
pub trait PeerStatsStore: Send + Sync {
    /// Charge toute la table (un appel au demarrage puis un flush
    /// periodique des seules entrees modifiees).
    fn load_peer_stats(&self) -> Vec<(Vec<u8>, PeerStat)>;
    /// Persiste une entree (upsert — appele sur les lignes
    /// modifiees depuis le dernier flush).
    fn upsert_peer_stat(&self, public_key: &[u8], stat: &PeerStat);
}

/// Store volatile : comportement identique sans persistance (tests,
/// outils, `set_peer_stats_store` jamais appele).
#[derive(Debug, Default)]
pub struct InMemoryPeerStatsStore {
    stats: Mutex<Vec<(Vec<u8>, PeerStat)>>,
}

impl PeerStatsStore for InMemoryPeerStatsStore {
    fn load_peer_stats(&self) -> Vec<(Vec<u8>, PeerStat)> {
        self.stats.lock().unwrap().clone()
    }

    fn upsert_peer_stat(&self, public_key: &[u8], stat: &PeerStat) {
        let mut stats = self.stats.lock().unwrap();
        if let Some(row) = stats.iter_mut().find(|(pk, _)| pk == public_key) {
            row.1 = stat.clone();
        } else {
            stats.push((public_key.to_vec(), stat.clone()));
        }
    }
}

/// Ligne d'un instantane API (`GET /api/ipv8/tunnel/ledger`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerStatInfo {
    /// `mid` hex du pair (comme `CircuitInfo::verified_hops`).
    pub mid: String,
    /// Octets servis pour ce pair.
    pub bytes_served: u64,
    /// Octets transportes par ce pair pour nous.
    pub bytes_used: u64,
    /// Dette `served - used` (0 = crediteur ou quitte).
    pub deficit: u64,
    /// Circuits qu'il nous a demandes.
    pub circuits_served: u64,
    /// Circuits ou il a servi comme saut verifie.
    pub circuits_used: u64,
    /// Epoch secondes de premiere observation.
    pub first_seen: u64,
    /// Epoch secondes de derniere activite.
    pub last_seen: u64,
}

/// Instantane complet du ledger (`GET /api/ipv8/tunnel/ledger`) :
/// reglages effectifs, totaux toutes cles confondues, et les
/// `max_peers_api` plus gros comptes par volume total.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LedgerInfo {
    /// Collecte active.
    pub enabled: bool,
    /// Gate d'admission active.
    pub enforce: bool,
    /// Seuil `joined` de declenchement de la gate.
    pub soft_cap: usize,
    /// Dette maximale toleree (octets).
    pub max_deficit_bytes: u64,
    /// Octets servis, tous pairs.
    pub total_served: u64,
    /// Octets utilises, tous pairs.
    pub total_used: u64,
    /// Nombre de pairs suivis.
    pub peer_count: usize,
    /// Top comptes par `served + used` (borne `top`).
    pub peers: Vec<PeerStatInfo>,
}

/// Nombre maximal de comptes remontes dans `LedgerInfo::peers`.
const API_TOP_PEERS: usize = 64;

/// Etat interne du livre : compteurs par cle publique + ensemble des
/// cles modifiees depuis le dernier flush persistant.
struct BookState {
    stats: HashMap<Vec<u8>, PeerStat>,
    dirty: HashSet<Vec<u8>>,
}

/// Livre des comptes locaux : compteurs par pair, gate d'admission et
/// persistance paresseuse. Synchronisation interne (`Mutex`) — jamais
/// de lock sur `inner` de la communaute ici (ordre de verrouillage
/// garanti : `inner` → `ledger`, jamais l'inverse).
pub struct PeerStatsBook {
    cfg: LedgerConfig,
    /// `cfg.enabled` duplique en atomique : bascule a chaud via
    /// `POST /api/settings` (`tunnel_community/ledger_enabled`).
    enabled: AtomicBool,
    /// `cfg.enforce` duplique en atomique
    /// (`tunnel_community/ledger_enforce`).
    enforce: AtomicBool,
    /// Avertissement "table pleine" emis une fois (pas de tempete de
    /// logs sous un flot de cles fraiches).
    capacity_warned: AtomicBool,
    state: Mutex<BookState>,
    store: Mutex<Option<Arc<dyn PeerStatsStore>>>,
}

impl PeerStatsBook {
    /// Cree le livre depuis le store injecte (ou vide).
    pub fn new(cfg: LedgerConfig, store: Option<Arc<dyn PeerStatsStore>>) -> Self {
        let stats = store
            .as_ref()
            .map(|s| s.load_peer_stats())
            .unwrap_or_default()
            .into_iter()
            .collect();
        Self {
            enabled: AtomicBool::new(cfg.enabled),
            enforce: AtomicBool::new(cfg.enforce),
            capacity_warned: AtomicBool::new(false),
            cfg,
            state: Mutex::new(BookState {
                stats,
                dirty: HashSet::new(),
            }),
            store: Mutex::new(store),
        }
    }

    /// Injection post-construction (la communaute est creee avant que
    /// `core` n'ait sa base prete — meme couture que
    /// `GuardSet::attach_store`). Charge la table si le livre est vide.
    pub fn attach_store(&self, store: Arc<dyn PeerStatsStore>) {
        let loaded = store.load_peer_stats();
        {
            let mut st = self.state.lock().unwrap();
            if st.stats.is_empty() && !loaded.is_empty() {
                st.stats = loaded.into_iter().collect();
            }
        }
        *self.store.lock().unwrap() = Some(store);
    }

    /// `true` = la collecte tourne.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Bascule a chaud de la collecte (les compteurs sont conserves,
    /// simplement plus alimentes — la gate retombe de fait a "tout
    /// admis" puisqu'elle exige `enabled && enforce`).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// `true` = la gate d'admission est arme.
    pub fn is_enforce(&self) -> bool {
        self.enforce.load(Ordering::Relaxed)
    }

    /// Bascule a chaud de la gate d'admission.
    pub fn set_enforce(&self, enforce: bool) {
        self.enforce.store(enforce, Ordering::Relaxed);
    }

    /// Acces a un compte (tests, gate).
    pub fn stat(&self, public_key: &[u8]) -> Option<PeerStat> {
        self.state.lock().unwrap().stats.get(public_key).cloned()
    }

    /// Entree mutable du pair, creee si besoin (respect de la borne
    /// `max_peers` : une cle fraiche n'entre pas dans une table
    /// pleine — `None` alors).
    fn entry<'a>(&self, st: &'a mut BookState, public_key: &[u8]) -> Option<&'a mut PeerStat> {
        if !st.stats.contains_key(public_key) && st.stats.len() >= self.cfg.max_peers {
            if !self.capacity_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    max_peers = self.cfg.max_peers,
                    "peer_stats : table pleine — nouvelles cles non suivies"
                );
            }
            return None;
        }
        let now = crate::pex::epoch_secs();
        let e = st
            .stats
            .entry(public_key.to_vec())
            .or_insert_with(|| PeerStat {
                first_seen: now,
                ..PeerStat::default()
            });
        e.last_seen = now;
        st.dirty.insert(public_key.to_vec());
        Some(e)
    }

    /// `bytes` octets servis pour `public_key` (delta du tick ou du
    /// retrait d'un objet de routage joint).
    pub fn note_served(&self, public_key: &[u8], bytes: u64) {
        if !self.is_enabled() || bytes == 0 {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if let Some(e) = self.entry(&mut st, public_key) {
            e.bytes_served = e.bytes_served.saturating_add(bytes);
        }
    }

    /// `bytes` octets transportes par `public_key` sur nos circuits.
    pub fn note_used(&self, public_key: &[u8], bytes: u64) {
        if !self.is_enabled() || bytes == 0 {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if let Some(e) = self.entry(&mut st, public_key) {
            e.bytes_used = e.bytes_used.saturating_add(bytes);
        }
    }

    /// Un `create` de `public_key` admis (`circuits_served`).
    pub fn note_join_served(&self, public_key: &[u8]) {
        if !self.is_enabled() {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if let Some(e) = self.entry(&mut st, public_key) {
            e.circuits_served = e.circuits_served.saturating_add(1);
        }
    }

    /// `public_key` apparait comme saut verifie d'un de nos circuits
    /// (`circuits_used` — une fois par saut par circuit).
    pub fn note_used_hop(&self, public_key: &[u8]) {
        if !self.is_enabled() {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if let Some(e) = self.entry(&mut st, public_key) {
            e.circuits_used = e.circuits_used.saturating_add(1);
        }
    }

    /// Gate d'admission d'un `create` (ADR-0015 §4) : sous pression
    /// (`joined >= soft_cap`), la dette du demandeur doit rester sous
    /// `max_deficit_bytes`. Desactive ou hors pression = admis
    /// (comportement pyipv8 exact).
    pub fn admit(&self, public_key: &[u8], joined: usize) -> bool {
        if !self.is_enabled() || !self.is_enforce() || joined < self.cfg.soft_cap {
            return true;
        }
        let deficit = self
            .state
            .lock()
            .unwrap()
            .stats
            .get(public_key)
            .map_or(0, PeerStat::deficit);
        deficit <= self.cfg.max_deficit_bytes
    }

    /// Persiste les entrees modifiees depuis le dernier flush —
    /// appele par le tick de maintenance et avant arret.
    pub fn flush(&self) {
        let store = self.store.lock().unwrap().clone();
        let Some(store) = store else { return };
        let batch: Vec<(Vec<u8>, PeerStat)> = {
            let mut st = self.state.lock().unwrap();
            let dirty: Vec<Vec<u8>> = st.dirty.drain().collect();
            dirty
                .into_iter()
                .filter_map(|pk| st.stats.get(&pk).map(|s| (pk, s.clone())))
                .collect()
        };
        for (pk, stat) in batch {
            store.upsert_peer_stat(&pk, &stat);
        }
    }

    /// Instantane complet pour l'API : reglages effectifs, totaux et
    /// top comptes par volume (`API_TOP_PEERS`).
    pub fn info(&self) -> LedgerInfo {
        let st = self.state.lock().unwrap();
        let mut peers: Vec<PeerStatInfo> = st
            .stats
            .iter()
            .map(|(pk, s)| PeerStatInfo {
                mid: hex::encode(onionbit_crypto::hash::ipv8_mid(pk)),
                bytes_served: s.bytes_served,
                bytes_used: s.bytes_used,
                deficit: s.deficit(),
                circuits_served: s.circuits_served,
                circuits_used: s.circuits_used,
                first_seen: s.first_seen,
                last_seen: s.last_seen,
            })
            .collect();
        peers.sort_by_key(|i| std::cmp::Reverse(i.bytes_served.saturating_add(i.bytes_used)));
        peers.truncate(API_TOP_PEERS);
        LedgerInfo {
            enabled: self.is_enabled(),
            enforce: self.is_enforce(),
            soft_cap: self.cfg.soft_cap,
            max_deficit_bytes: self.cfg.max_deficit_bytes,
            total_served: st.stats.values().map(|s| s.bytes_served).sum(),
            total_used: st.stats.values().map(|s| s.bytes_used).sum(),
            peer_count: st.stats.len(),
            peers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(enforce: bool) -> PeerStatsBook {
        PeerStatsBook::new(
            LedgerConfig {
                enabled: true,
                enforce,
                soft_cap: 4,
                max_deficit_bytes: 100,
                tick: Duration::from_secs(30),
                max_peers: 8,
            },
            None,
        )
    }

    #[test]
    fn pair_inconnu_toujours_admis() {
        let b = book(true);
        assert!(b.admit(b"nouveau", 100), "dette nulle -> credit de depart");
    }

    #[test]
    fn deficit_bloque_seulement_sous_pression() {
        let b = book(true);
        b.note_served(b"freerider", 150);
        assert!(
            !b.admit(b"freerider", 10),
            "dette 150 > credit 100, sous pression"
        );
        assert!(b.admit(b"freerider", 3), "sous soft_cap (3 < 4) -> admis");
        b.note_used(b"freerider", 60);
        assert!(b.admit(b"freerider", 10), "dette 90 <= credit -> admis");
    }

    #[test]
    fn crediteur_jamais_bloque() {
        let b = book(true);
        b.note_used(b"genereux", 1000);
        assert!(b.admit(b"genereux", 100), "deficit sature a 0");
    }

    #[test]
    fn sans_enforce_tout_est_pyipv8() {
        let b = book(false);
        b.note_served(b"freerider", u64::MAX / 2);
        assert!(b.admit(b"freerider", usize::MAX / 2));
    }

    #[test]
    fn collecte_desactivee_ne_compte_pas() {
        let b = PeerStatsBook::new(
            LedgerConfig {
                enabled: false,
                enforce: true,
                ..LedgerConfig::default()
            },
            None,
        );
        b.note_served(b"x", 42);
        b.note_join_served(b"x");
        assert!(b.stat(b"x").is_none(), "collecte off -> aucune entree");
        assert!(b.admit(b"x", usize::MAX / 2));
    }

    #[test]
    fn table_bornee_ne_croit_pas_sans_fin() {
        let b = book(false);
        for i in 0..8u8 {
            b.note_served(&[i], 1);
        }
        b.note_served(&[9], 1);
        assert_eq!(b.info().peer_count, 8, "max_peers respecte");
    }

    #[test]
    fn flush_ne_persiste_que_le_dirty() {
        let store = Arc::new(InMemoryPeerStatsStore::default());
        let b = PeerStatsBook::new(
            LedgerConfig {
                max_peers: 8,
                ..LedgerConfig::default()
            },
            Some(store.clone() as Arc<dyn PeerStatsStore>),
        );
        b.note_served(b"a", 10);
        b.note_used(b"b", 5);
        b.flush();
        assert_eq!(store.load_peer_stats().len(), 2);
        b.note_served(b"a", 1);
        b.flush();
        let rows = store.load_peer_stats();
        assert_eq!(rows.len(), 2, "upsert, pas de doublon");
        assert_eq!(
            rows.iter()
                .find(|(pk, _)| pk == b"a")
                .unwrap()
                .1
                .bytes_served,
            11
        );
    }

    #[test]
    fn chargement_depuis_store_au_boot() {
        let store = Arc::new(InMemoryPeerStatsStore::default());
        store.upsert_peer_stat(
            b"ancien",
            &PeerStat {
                bytes_served: 7,
                ..PeerStat::default()
            },
        );
        let b = PeerStatsBook::new(
            LedgerConfig::default(),
            Some(store as Arc<dyn PeerStatsStore>),
        );
        assert_eq!(b.stat(b"ancien").unwrap().bytes_served, 7);
    }
}
