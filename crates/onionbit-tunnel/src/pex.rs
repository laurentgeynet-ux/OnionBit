// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Store PEX par `info_hash` — l'equivalent donnees de
//! `PexCommunity` (`messaging/anonymization/pex.py`).
//!
//! Python attache une `PexCommunity` complete (overlay avec ses
//! strategies `RandomWalk`/`RandomChurn` et l'apprentissage via
//! `process_extra_bytes`) a chaque `info_hash` pour lequel on heberge
//! un point d'introduction. L'overlay propre n'est pas porte ; ce
//! store conserve la shape `pex[info_hash].get_intro_points()`
//! attendue par `on_peers_request` et `/api/ipv8/tunnel/peers/pex`.

use std::collections::VecDeque;

use onionbit_ipv8::UdpAddress;

use crate::routing::{IntroductionPoint, PEER_SOURCE_PEX};

/// `deque(maxlen=20)` de `PexCommunity.intro_points`.
pub const PEX_MAX_INTRO_POINTS: usize = 20;
/// Eviction `last_seen + 300 < now` (`get_intro_points`, secondes).
pub const PEX_INTRO_POINT_TTL_SECS: u64 = 300;

/// `time.time()` en secondes (epoch) — les `last_seen` de
/// `IntroductionPoint` sont comparees a l'horloge murale comme en
/// Python (le store survit aux monotones de circuits).
pub(crate) fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// `PexCommunity` reduite a son stockage.
#[derive(Debug, Default)]
pub(crate) struct PexStore {
    /// `intro_points` : points appris via PEX — les plus recents en
    /// tete (`appendleft`), bornes a `PEX_MAX_INTRO_POINTS`.
    /// Vide en l'absence de l'overlay `PexCommunity` propre.
    pub intro_points: VecDeque<IntroductionPoint>,
    /// `intro_points_for` : `seeder_pk` que l'on annonce soi-meme
    /// (`start_announce` lors d'`on_establish_intro`).
    pub intro_points_for: Vec<Vec<u8>>,
}

impl PexStore {
    /// `get_intro_points` : evince les points perimes (les plus
    /// anciens en queue — `pop()` sur un `deque` rempli par
    /// `appendleft`), puis concatene avec nos propres annonces
    /// (`IntroductionPoint(my_peer, seeder_pk, PEER_SOURCE_PEX)`).
    pub fn intro_points(
        &mut self,
        our_key: &[u8],
        our_wan: &UdpAddress,
        now: u64,
    ) -> Vec<IntroductionPoint> {
        while self
            .intro_points
            .back()
            .is_some_and(|ip| now > ip.last_seen_secs.saturating_add(PEX_INTRO_POINT_TTL_SECS))
        {
            self.intro_points.pop_back();
        }
        let mut ips: Vec<IntroductionPoint> = self.intro_points.iter().cloned().collect();
        ips.extend(
            self.intro_points_for
                .iter()
                .map(|seeder_pk| IntroductionPoint {
                    address: our_wan.clone(),
                    peer_key: our_key.to_vec(),
                    seeder_pk: seeder_pk.clone(),
                    source: PEER_SOURCE_PEX,
                    last_seen_secs: now,
                }),
        );
        ips
    }

    /// `process_extra_bytes` : apprend un point d'introduction (le
    /// plus recent passe en tete, borne a `PEX_MAX_INTRO_POINTS`).
    #[allow(dead_code)] // appele par l'overlay PEX quand il sera porte
    pub fn push_learned(&mut self, ip: IntroductionPoint) {
        self.intro_points.retain(|i| i != &ip);
        self.intro_points.push_front(ip);
        self.intro_points.truncate(PEX_MAX_INTRO_POINTS);
    }

    /// `start_announce` : on s'annonce point d'introduction du
    /// `seeder_pk` (idempotent).
    pub fn start_announce(&mut self, seeder_pk: Vec<u8>) {
        if !self.intro_points_for.contains(&seeder_pk) {
            self.intro_points_for.push(seeder_pk);
        }
    }

    /// `stop_announce` : on cesse d'annoncer ce `seeder_pk`.
    pub fn stop_announce(&mut self, seeder_pk: &[u8]) {
        self.intro_points_for.retain(|p| p.as_slice() != seeder_pk);
    }

    /// `done` : plus rien a annoncer → le store est dechargeable.
    pub fn is_done(&self) -> bool {
        self.intro_points_for.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(pk: u8, seeder: u8, last_seen: u64) -> IntroductionPoint {
        IntroductionPoint {
            address: UdpAddress::unspecified(),
            peer_key: vec![pk],
            seeder_pk: vec![seeder],
            source: PEER_SOURCE_PEX,
            last_seen_secs: last_seen,
        }
    }

    /// `start_announce`/`get_intro_points` : nos annonces sont
    /// retournees avec `source` PEX ; `stop_announce` → `done`.
    #[test]
    fn announce_cycle() {
        let mut store = PexStore::default();
        store.start_announce(vec![1]);
        store.start_announce(vec![1]);
        assert_eq!(store.intro_points_for.len(), 1, "annonce idempotente");
        assert!(!store.is_done());
        let ips = store.intro_points(b"me", &UdpAddress::unspecified(), 1000);
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0].seeder_pk, vec![1]);
        assert_eq!(ips[0].source, PEER_SOURCE_PEX);
        assert_eq!(ips[0].peer_key, b"me".to_vec());
        store.stop_announce(&[1]);
        assert!(store.is_done());
        assert!(store
            .intro_points(b"me", &UdpAddress::unspecified(), 1000)
            .is_empty());
    }

    /// `get_intro_points` evince les points appris dont
    /// `last_seen + 300 < now` (les plus anciens en queue).
    #[test]
    fn learned_ttl_eviction() {
        let mut store = PexStore::default();
        store.push_learned(ip(1, 9, 100));
        store.push_learned(ip(2, 9, 5000));
        let ips = store.intro_points(b"me", &UdpAddress::unspecified(), 5000);
        // 5000 > 100 + 300 → l'ancien est evince, le recent reste.
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0].peer_key, vec![2]);
    }

    /// `deque(maxlen=20)` : au-dela, les plus anciens tombent.
    #[test]
    fn learned_capacity() {
        let mut store = PexStore::default();
        for i in 0..25u8 {
            store.push_learned(ip(i, i, 5000));
        }
        assert_eq!(store.intro_points.len(), PEX_MAX_INTRO_POINTS);
        // Le plus recent (24) en tete ; le plus ancien (0..4) evince.
        assert_eq!(store.intro_points.front().unwrap().peer_key, vec![24]);
    }
}
