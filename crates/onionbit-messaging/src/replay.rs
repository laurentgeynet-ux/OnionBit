// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Anti-replay / ordre / doublons (ADR-0011) : fenetre de reception
//! sur `seq` (modele IPSec — bitmap des dernieres sequences) +
//! deduplication par `id` de trame pour absorber les reemissions
//! honnetes (circuit e2e reconstruit).

use std::collections::{HashSet, VecDeque};

use crate::config::MessagingConfig;
use crate::error::MessagingError;
use crate::frame::MSG_ID_LEN;

/// Fenetre de reception d'une conversation (un `RecvWindow` par
/// contact/direction).
///
/// - `seq > top` : accepte, la fenetre avance.
/// - `top - seq < window` : accepte si le bit est libre (desordre
///   tolerer dans la fenetre), rejete si deja recu.
/// - `seq <= top - window` : trop ancien, rejete.
///
/// Un `id` deja vu est toujours rejete (`DuplicateId`) — une
/// trame rejouee sous un `seq` neuf n'entre jamais deux fois dans
/// l'historique.
#[derive(Debug)]
pub struct RecvWindow {
    /// Plus grand `seq` accepte (`None` avant la premiere trame).
    top: Option<u64>,
    /// Bitmap des `window` derniers `seq` : bit `i` = `(top - i)`
    /// recu.
    bitmap: u64,
    /// Largeur de fenetre (borne le desordre admis).
    window: u32,
    /// `id`s des trames acceptees (dedup).
    seen: HashSet<[u8; MSG_ID_LEN]>,
    /// Ordre d'arrivee des `id`s pour l'eviction FIFO.
    order: VecDeque<[u8; MSG_ID_LEN]>,
    /// Capacite du cache de dedup.
    dedup_cap: usize,
}

impl RecvWindow {
    /// Nouvelle fenetre selon la configuration.
    pub fn new(cfg: &MessagingConfig) -> Self {
        Self {
            top: None,
            bitmap: 0,
            window: cfg.recv_window.min(63),
            seen: HashSet::new(),
            order: VecDeque::new(),
            dedup_cap: cfg.dedup_cap,
        }
    }

    /// Admet ou rejette une trame `(seq, id)`.
    ///
    /// A appeler **apres** `Frame::open` (authenticite prouvee) :
    /// une trame non authentifiee ne doit jamais consommer de
    /// fenetre ni de `id`.
    pub fn admit(&mut self, seq: u64, id: &[u8; MSG_ID_LEN]) -> Result<(), MessagingError> {
        if self.seen.contains(id) {
            return Err(MessagingError::DuplicateId);
        }
        match self.top {
            Some(top) if seq <= top => {
                let age = top - seq;
                if age >= self.window as u64 {
                    return Err(MessagingError::TooOld);
                }
                let bit = 1u64 << age;
                if self.bitmap & bit != 0 {
                    return Err(MessagingError::Replayed);
                }
                self.bitmap |= bit;
            }
            Some(top) => {
                // seq > top : la fenetre avance (saturer le decalage
                // — un saut au-dela de 64 vide simplement le bitmap).
                let shift = (seq - top).min(64);
                self.bitmap = (self.bitmap << shift) | 1;
                self.top = Some(seq);
            }
            None => {
                self.top = Some(seq);
                self.bitmap = 1;
            }
        }
        if self.seen.insert(*id) {
            self.order.push_back(*id);
            if self.order.len() > self.dedup_cap {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> RecvWindow {
        RecvWindow::new(&MessagingConfig::default())
    }

    /// Croissance monotone : chaque `seq` superieur est accepte.
    #[test]
    fn seq_croissants_acceptes() {
        let mut w = window();
        for seq in 0..10 {
            w.admit(seq, &[seq as u8; MSG_ID_LEN]).unwrap();
        }
    }

    /// Rejeu exact de `seq` rejete ; desordre dans la fenetre
    /// tolere une fois.
    #[test]
    fn rejeu_rejete_desordre_tolere() {
        let mut w = window();
        w.admit(10, &[1; MSG_ID_LEN]).unwrap();
        w.admit(12, &[2; MSG_ID_LEN]).unwrap();
        // Desordre : 11 arrive apres 12 — accepte une fois.
        w.admit(11, &[3; MSG_ID_LEN]).unwrap();
        assert!(matches!(
            w.admit(11, &[4; MSG_ID_LEN]),
            Err(MessagingError::Replayed)
        ));
        assert!(matches!(
            w.admit(10, &[5; MSG_ID_LEN]),
            Err(MessagingError::Replayed)
        ));
    }

    /// `seq` hors fenetre (trop ancien) rejete.
    #[test]
    fn trop_ancien_rejete() {
        let mut w = window();
        w.admit(1000, &[9; MSG_ID_LEN]).unwrap();
        assert!(matches!(
            w.admit(900, &[8; MSG_ID_LEN]),
            Err(MessagingError::TooOld)
        ));
    }

    /// Un `id` duplique est rejete meme sous un `seq` neuf
    /// (reemission honnete sur un circuit reconstruit avec un
    /// compteur continue : la dedup `id` couvre ce que `seq` ne
    /// voit pas).
    #[test]
    fn id_duplique_rejete() {
        let mut w = window();
        let id = [42u8; MSG_ID_LEN];
        w.admit(1, &id).unwrap();
        assert!(matches!(w.admit(2, &id), Err(MessagingError::DuplicateId)));
    }

    /// Le cache de dedup est borne (eviction FIFO au-dela de
    /// `dedup_cap`).
    #[test]
    fn dedup_borne() {
        let cfg = MessagingConfig {
            dedup_cap: 4,
            ..Default::default()
        };
        let mut w = RecvWindow::new(&cfg);
        for i in 0..4u8 {
            w.admit(i as u64, &[i; MSG_ID_LEN]).unwrap();
        }
        // Le premier `id` est evince au-dela de la capacite : un
        // rejeu de celui-ci n'est plus vu comme duplique mais tombe
        // sur la fenetre `seq` — qui tient toujours (age 7 < 64).
        for i in 4..8u8 {
            w.admit(i as u64, &[i; MSG_ID_LEN]).unwrap();
        }
        assert!(matches!(
            w.admit(0, &[0u8; MSG_ID_LEN]),
            Err(MessagingError::Replayed)
        ));
    }
}
