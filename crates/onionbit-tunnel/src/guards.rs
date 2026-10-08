// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Guard nodes : persistance du premier saut (ADR-0010).
//!
//! pyipv8 tire chaque premier saut dans tout le pool de relais : chaque
//! circuit construit est un tirage Sybil, et un storm de `DESTROY`
//! multiplie les chances de tomber sur un noeud d'entree hostile qui
//! voit l'IP reelle. Les guards bornent ce tirage : un petit ensemble
//! persistant de premiers sauts est reutilise jusqu'a expiration ou
//! echec prolonge.
//!
//! Ecart pyipv8 : **comportemental uniquement** — aucun champ de
//! protocole ne change ; les relais Tribler ignores qu'ils servent de
//! guard. `GuardsConfig::enabled = false` restaure la selection exacte
//! pyipv8. Jamais applique aux circuits a `hops == 0` (il n'y en a pas
//! : `anon_engine` rejette 0).

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use onionbit_ipv8::{Peer, UdpAddress};

/// Reglages des guard nodes (aucune valeur en dur — tout est ici).
#[derive(Debug, Clone)]
pub struct GuardsConfig {
    /// `true` = premiers sauts persistants ; `false` = tirage pyipv8
    /// exact (repli diagnostic/interop). Defaut crate `false`
    /// (opt-in du struct) — le daemon l'active par defaut via
    /// `tunnel_community/guards_enabled = true` depuis la validation
    /// terrain (ADR-0010, statut Acceptee).
    pub enabled: bool,
    /// Nombre de guards actifs (3, comme `NumEntryGuards` historique
    /// de Tor — repartit sans diluer la persistance).
    pub active_count: usize,
    /// Guards de reserve : remplacants prets avant un tirage sous
    /// pression (2).
    pub reserve_count: usize,
    /// Duree de vie d'un guard (30 jours — entre le `GuardLifetime` de
    /// Tor ~4 mois et l'absence de persistance pyipv8).
    pub lifetime: Duration,
    /// Delai sans preuve de vie au-dela duquel un guard est retrograde
    /// en reserve puis remplace (24 h).
    pub unreachable_after: Duration,
    /// Echecs de handshake `create` consecutifs avant retrogradation
    /// immediate — independante de l'horloge (3).
    pub max_failures: u32,
    /// Cadence de la maintenance proactive du set (purge des expires,
    /// adoptions anticipees sur le pool courant) — hors construction
    /// de circuit, pour ne jamais adopter sous la pression d'un
    /// storm (60 s).
    pub maintenance_interval: Duration,
}

impl Default for GuardsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_count: 3,
            reserve_count: 2,
            lifetime: Duration::from_secs(30 * 24 * 60 * 60),
            unreachable_after: Duration::from_secs(24 * 60 * 60),
            max_failures: 3,
            maintenance_interval: Duration::from_secs(60),
        }
    }
}

/// Etat persistable d'un guard — identifie par sa **cle publique**
/// (un changement d'adresse conserve le guard ; un changement de cle
/// produit un nouveau candidat).
#[derive(Debug, Clone)]
pub struct GuardRecord {
    /// `key_to_bin()` du pair.
    pub public_key_bin: Vec<u8>,
    /// Derniere adresse ayant repondu — rafraichie a chaque handshake.
    pub last_address: Option<UdpAddress>,
    /// Date d'adoption (rotation a `lifetime`).
    pub adopted_at: SystemTime,
    /// Derniere preuve de vie (`created` reussi).
    pub last_seen: SystemTime,
    /// Echecs de handshake `create` consecutifs.
    pub failures: u32,
    /// `false` = actif, `true` = reserve.
    pub reserve: bool,
}

/// Persistance injectable — `onionbit-tunnel` ne depend pas de
/// `onionbit-db` (sens des dependances inverse). Implementee par la
/// table `guards` cote `onionbit-db`, injectee par `core`/`daemon`.
pub trait GuardStore: Send + Sync {
    /// Charge le set persistant (ordre = anciennete d'adoption).
    fn load_guards(&self) -> Vec<GuardRecord>;
    /// Persiste le set complet (5 enregistrements — cout trivial).
    fn save_guards(&self, guards: &[GuardRecord]);
}

/// Store volatile : comportement identique sans persistance (tests,
/// outils, `set_guard_store` jamais appele).
#[derive(Debug, Default)]
pub struct InMemoryGuardStore {
    records: Mutex<Vec<GuardRecord>>,
}

impl GuardStore for InMemoryGuardStore {
    fn load_guards(&self) -> Vec<GuardRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn save_guards(&self, guards: &[GuardRecord]) {
        *self.records.lock().unwrap_or_else(|e| e.into_inner()) = guards.to_vec();
    }
}

/// Instantane d'un guard (`/api/ipv8/tunnel/guards`) — diagnostic
/// utilisateur : le mid hex tronque masque la cle complete (la liste
/// des premiers sauts est revelatrice de topologie, cf. ADR-0010 §UX).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GuardInfo {
    /// `mid` hex du pair (comme `CircuitInfo::verified_hops`).
    pub mid: String,
    /// Derniere adresse connue `"ip:port"` (`""` = inconnue).
    pub address: String,
    /// `true` = reserve, `false` = actif.
    pub reserve: bool,
    /// Echecs de handshake `create` consecutifs.
    pub failures: u32,
    /// Epoch secondes de l'adoption.
    pub adopted_at: u64,
    /// Epoch secondes de la derniere preuve de vie.
    pub last_seen: u64,
}

/// `true` si deux adresses sont dans le meme sous-reseau (/24 IPv4,
/// /64 IPv6) ou identiques — dedup de diversite a l'admission.
/// Les domaines sont comparees par nom exact ; sans adresse connue on
/// ne peut pas trancher (`false` = admissible).
fn same_network(a: &UdpAddress, b: &UdpAddress) -> bool {
    if a == b {
        return true;
    }
    match (a.to_socket_addr(), b.to_socket_addr()) {
        (Some(sa), Some(sb)) => match (sa.ip(), sb.ip()) {
            (IpAddr::V4(x), IpAddr::V4(y)) => x.octets()[..3] == y.octets()[..3],
            (IpAddr::V6(x), IpAddr::V6(y)) => x.octets()[..8] == y.octets()[..8],
            _ => false,
        },
        _ => false,
    }
}

/// Ensemble de guards d'une communaute : selection + sante + rotation.
/// Synchronisation interne (`Mutex`) — jamais de lock sur `inner` de
/// la communaute ici.
pub struct GuardSet {
    cfg: GuardsConfig,
    /// `cfg.enabled` duplique en atomique : bascule a chaud via
    /// `POST /api/settings` (`tunnel_community/guards_enabled`) sans
    /// reconstruire la communaute.
    enabled: AtomicBool,
    records: Mutex<Vec<GuardRecord>>,
    store: Mutex<Option<Arc<dyn GuardStore>>>,
}

impl GuardSet {
    /// Cree le set depuis le store injecte (ou vide).
    pub fn new(cfg: GuardsConfig, store: Option<Arc<dyn GuardStore>>) -> Self {
        let records = store.as_ref().map(|s| s.load_guards()).unwrap_or_default();
        Self {
            enabled: AtomicBool::new(cfg.enabled),
            cfg,
            records: Mutex::new(records),
            store: Mutex::new(store),
        }
    }

    /// `true` = premieres hops ordonnees par le set de guards.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Bascule a chaud : `false` = selection pyipv8 exacte (le set
    /// persistant conserve, reutilise au re-armement).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Injection post-construction (la communaute est creee avant que
    /// `core` n'ait son store DB pret). Charge le set persistant si
    /// aucun record n'existe deja.
    pub fn attach_store(&self, store: Arc<dyn GuardStore>) {
        let loaded = store.load_guards();
        {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            if records.is_empty() && !loaded.is_empty() {
                *records = loaded;
            }
        }
        *self.store.lock().unwrap_or_else(|e| e.into_inner()) = Some(store);
    }

    /// Instantane des cles des guards (actifs puis reserve) — pour
    /// l'API de diagnostic et les tests.
    pub fn guard_keys(&self) -> Vec<Vec<u8>> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.public_key_bin.clone())
            .collect()
    }

    /// Instantane diagnostic du set (actifs puis reserve) —
    /// `GuardInfo` pour l'API (`/api/ipv8/tunnel/guards`).
    pub fn guards_info(&self) -> Vec<GuardInfo> {
        let epoch = |t: SystemTime| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        };
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|r| GuardInfo {
                mid: hex::encode(onionbit_crypto::hash::ipv8_mid(&r.public_key_bin)),
                address: r
                    .last_address
                    .as_ref()
                    .and_then(|a| a.to_socket_addr())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                reserve: r.reserve,
                failures: r.failures,
                adopted_at: epoch(r.adopted_at),
                last_seen: epoch(r.last_seen),
            })
            .collect()
    }

    /// Persiste l'etat courant si un store est injecte.
    fn persist(&self, records: &[GuardRecord]) {
        if let Some(s) = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            s.save_guards(records);
        }
    }

    /// Alimente le set depuis les candidats disponibles et retire les
    /// guards expires/injoignables. Appele avant chaque selection —
    /// la rotation ne se fait jamais sous pression (storm).
    pub fn ensure(&self, candidates: &[Peer]) {
        let now = SystemTime::now();
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());

        // 1. Expiration naturelle + injoignabilite prolongee.
        let unreachable = self.cfg.unreachable_after;
        let lifetime = self.cfg.lifetime;
        records.retain(|r| {
            now.duration_since(r.adopted_at).unwrap_or_default() < lifetime
                && now.duration_since(r.last_seen).unwrap_or_default() < unreachable
        });
        // 2. Un reserve devient actif pour remplacer un actif perdu.
        for i in 0..records.len() {
            if records[i].reserve
                && records.iter().filter(|x| !x.reserve).count() < self.cfg.active_count
            {
                records[i].reserve = false;
            }
        }
        // 3. Adoption : actifs puis reserve, avec dedup de diversite.
        let adopt = |records: &mut Vec<GuardRecord>, want_reserve: bool, max: usize| {
            while records.iter().filter(|r| r.reserve == want_reserve).count() < max {
                let Some(c) = candidates.iter().find(|p| {
                    !records.iter().any(|r| r.public_key_bin == p.public_key_bin)
                        && !records.iter().any(|r| match (&r.last_address, &p.address) {
                            (Some(ra), Some(pa)) => same_network(ra, pa),
                            _ => false,
                        })
                }) else {
                    break;
                };
                records.push(GuardRecord {
                    public_key_bin: c.public_key_bin.clone(),
                    last_address: c.address.clone(),
                    adopted_at: now,
                    last_seen: now,
                    failures: 0,
                    reserve: want_reserve,
                });
            }
        };
        let (active_max, reserve_max) = (self.cfg.active_count, self.cfg.reserve_count);
        adopt(&mut records, false, active_max);
        adopt(&mut records, true, reserve_max);
        if records.is_empty() && !candidates.is_empty() {
            tracing::warn!("guards_pool_etroit : aucun guard adoptable dans le pool");
        }
        let snapshot = records.clone();
        drop(records);
        self.persist(&snapshot);
    }

    /// Liste ordonnee des premiers hops : actifs ++ reserve ++
    /// `candidates` restants (deja tries par frequence/brasses par
    /// l'appelant). Les guards absents du reseau gardent leur
    /// `last_address` — un `create` vers une vieille adresse echoue
    /// comme un timeout normal, sans cout special.
    pub fn order_first_hops(&self, candidates: Vec<Peer>) -> Vec<Peer> {
        if !self.is_enabled() {
            return candidates;
        }
        let records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Peer> = Vec::with_capacity(candidates.len() + records.len());
        for r in records.iter() {
            if let Some(p) = candidates
                .iter()
                .find(|p| p.public_key_bin == r.public_key_bin)
                .cloned()
                .or_else(|| Peer::new(r.public_key_bin.clone(), r.last_address.clone()))
            {
                out.push(p);
            }
        }
        for p in candidates {
            if !out.iter().any(|q| q.public_key_bin == p.public_key_bin) {
                out.push(p);
            }
        }
        out
    }

    /// Handshake `create` reussi vers `key` : preuve de vie + adresse
    /// rafraichie (le guard peut avoir change d'IP — la cle reste).
    pub fn mark_alive(&self, key: &[u8], address: Option<UdpAddress>) {
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = records.iter_mut().find(|r| r.public_key_bin == key) {
            r.last_seen = SystemTime::now();
            r.failures = 0;
            if address.is_some() {
                r.last_address = address;
            }
            let snapshot = records.clone();
            drop(records);
            self.persist(&snapshot);
        }
    }

    /// Timeout du handshake `create` vers `key` : compte l'echec ;
    /// a `max_failures` le guard est retrograde en fin de reserve —
    /// un actif de reserve le remplace au prochain `ensure`, jamais
    /// en plein retry de circuit.
    pub fn mark_failure(&self, key: &[u8]) {
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pos) = records.iter().position(|r| r.public_key_bin == key) else {
            return;
        };
        records[pos].failures += 1;
        if records[pos].failures >= self.cfg.max_failures {
            let mut demoted = records.remove(pos);
            demoted.reserve = true;
            demoted.failures = 0;
            // Fin de reserve : les guards eprouves gardent leur rang.
            records.push(demoted);
            tracing::debug!(
                key = ?&key[..key.len().min(8)],
                "guard retrograde en reserve (echecs de handshake)"
            );
            // Un reserve devient actif immediatement si possible.
            if records.iter().filter(|r| !r.reserve).count() < self.cfg.active_count {
                if let Some(next) = records.iter_mut().find(|r| r.reserve) {
                    next.reserve = false;
                }
            }
        }
        let snapshot = records.clone();
        drop(records);
        self.persist(&snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pair de test : cle publique pseudo-LibNaCl arbitrairement
    /// formatee n'est pas requise — `Peer::new` l'exige, donc on passe
    /// par une vraie cle generee.
    fn peer_at(ip: [u8; 4], port: u16) -> Peer {
        let sk = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
        Peer::new(
            sk.public_key().to_bin(),
            Some(UdpAddress::from(std::net::SocketAddr::new(
                IpAddr::V4(std::net::Ipv4Addr::from(ip)),
                port,
            ))),
        )
        .unwrap()
    }

    fn cfg() -> GuardsConfig {
        GuardsConfig {
            enabled: true,
            ..GuardsConfig::default()
        }
    }

    #[test]
    fn adoption_trois_actifs_deux_reserve_avec_diversite() {
        let set = GuardSet::new(cfg(), None);
        // 7 candidats : deux dans le meme /24 — un seul admissible.
        let cands: Vec<Peer> = vec![
            peer_at([10, 0, 0, 1], 7000),
            peer_at([10, 0, 0, 2], 7001), // meme /24 que le precedent
            peer_at([10, 0, 1, 1], 7002),
            peer_at([10, 0, 2, 1], 7003),
            peer_at([10, 0, 3, 1], 7004),
        ];
        set.ensure(&cands);
        assert_eq!(
            set.guard_keys().len(),
            4,
            "3 actifs + 1 reserve (/24 dedup)"
        );
    }

    #[test]
    fn ordre_guards_puis_reserve_puis_libre() {
        let set = GuardSet::new(cfg(), None);
        let cands: Vec<Peer> = (0..8).map(|i| peer_at([10, 0, i, 1], 7000)).collect();
        set.ensure(&cands);
        let ordered = set.order_first_hops(cands.clone());
        let guards = set.guard_keys();
        for (i, g) in guards.iter().enumerate() {
            assert_eq!(&ordered[i].public_key_bin, g, "guard {i} en tete");
        }
        assert_eq!(ordered.len(), cands.len());
    }

    #[test]
    fn desactive_est_le_tirage_pyipv8_exact() {
        let set = GuardSet::new(GuardsConfig::default(), None);
        let cands: Vec<Peer> = (0..4).map(|i| peer_at([10, 0, i, 1], 7000)).collect();
        let ordered = set.order_first_hops(cands.clone());
        assert_eq!(
            ordered
                .iter()
                .map(|p| &p.public_key_bin)
                .collect::<Vec<_>>(),
            cands.iter().map(|p| &p.public_key_bin).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn bascule_a_chaud_sans_reconstruction() {
        // `POST /api/settings` (`tunnel_community/guards_enabled`) :
        // le set persistant est conserve entre les bascules.
        let set = GuardSet::new(cfg(), None);
        let cands: Vec<Peer> = (0..8).map(|i| peer_at([10, 0, i, 1], 7000)).collect();
        set.ensure(&cands);
        let guards = set.guard_keys();
        assert_eq!(
            set.order_first_hops(cands.clone())[0].public_key_bin,
            guards[0],
            "guard en tete quand active"
        );
        set.set_enabled(false);
        assert_eq!(
            set.order_first_hops(cands.clone())
                .iter()
                .map(|p| p.public_key_bin.clone())
                .collect::<Vec<_>>(),
            cands
                .iter()
                .map(|p| p.public_key_bin.clone())
                .collect::<Vec<_>>(),
            "tirage pyipv8 exact quand desactive a chaud"
        );
        set.set_enabled(true);
        assert_eq!(
            set.order_first_hops(cands.clone())[0].public_key_bin,
            guards[0],
            "meme guard apres re-armement (set conserve)"
        );
    }

    #[test]
    fn echecs_retrogradent_sans_tirage_sous_pression() {
        let set = GuardSet::new(cfg(), None);
        let cands: Vec<Peer> = (0..8).map(|i| peer_at([10, 0, i, 1], 7000)).collect();
        set.ensure(&cands);
        let first = set.guard_keys()[0].clone();
        for _ in 0..3 {
            set.mark_failure(&first);
        }
        assert_ne!(
            set.guard_keys()[0],
            first,
            "guard en echec n'est plus en tete"
        );
        // Le set est borne : aucun tirage nouveau n'a ete fait.
        assert!(set.guard_keys().len() <= 5);
    }

    #[test]
    fn preuve_de_vie_remet_les_compteurs_a_zero() {
        let set = GuardSet::new(cfg(), None);
        let cands: Vec<Peer> = (0..8).map(|i| peer_at([10, 0, i, 1], 7000)).collect();
        set.ensure(&cands);
        let first = set.guard_keys()[0].clone();
        set.mark_failure(&first);
        set.mark_failure(&first);
        set.mark_alive(&first, cands[0].address.clone());
        set.mark_failure(&first);
        set.mark_failure(&first);
        assert_eq!(set.guard_keys()[0], first, "vivant apres remise a zero");
    }
}
