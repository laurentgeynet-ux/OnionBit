// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Machine d'etat anti-rejeu `RecvWindow` (ADR-0011) : fuzzing
// DIFFERENTIEL — chaque sequence d'operations (`admit`, `resume`,
// reset, reconfiguration) est comparee a un modele de reference
// trivial (ensemble de seqs vus + cache FIFO d'`id`). Une
// divergence signifie qu'un rejeu passe, qu'une trame honnete est
// refusee, ou qu'une frontiere de decalage casse le bitmap.
//
// Surface que `messaging_frame` ne couvre pas : `admit` est hors
// de `Frame::open` — c'est ici que vivent `top`, le bitmap 64
// bits, les transitions Replayed/TooOld/DuplicateId et la reprise
// `resume` apres restart.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_messaging::frame::MSG_ID_LEN;
use onionbit_messaging::{MessagingConfig, MessagingError, RecvWindow};
use std::collections::{HashSet, VecDeque};

/// Frontieres de sequence a exercer explicitement.
const BOUNDARIES: &[u64] = &[
    0,
    1,
    62,
    63,
    64,
    65,
    u32::MAX as u64 - 1,
    u32::MAX as u64,
    u32::MAX as u64 + 1,
    u64::MAX - 1,
    u64::MAX,
];

/// Domaine d'`id` volontairement petit : avec un `dedup_cap`
/// borne, les collisions et evictions sont frequentes.
fn mk_id(b: u8) -> [u8; MSG_ID_LEN] {
    [b; MSG_ID_LEN]
}

/// Modele de reference : sémantique attendue de `RecvWindow`,
/// sans bitmap — un ensemble borne de seqs vus.
struct Model {
    top: Option<u64>,
    /// Seqs acceptes encore dans la fenetre courante.
    seen: HashSet<u64>,
    ids: HashSet<u8>,
    order: VecDeque<u8>,
    window: u64,
    dedup_cap: usize,
}

impl Model {
    fn new(cfg: &MessagingConfig) -> Self {
        Self {
            top: None,
            seen: HashSet::new(),
            ids: HashSet::new(),
            order: VecDeque::new(),
            window: u64::from(cfg.recv_window.min(63)),
            dedup_cap: cfg.dedup_cap,
        }
    }

    fn resume(&mut self, cfg: &MessagingConfig, top: u64) {
        *self = Self::new(cfg);
        self.top = Some(top);
        self.seen.insert(top);
    }

    fn admit(&mut self, seq: u64, id: u8) -> Result<(), u8> {
        if self.ids.contains(&id) {
            return Err(3); // DuplicateId
        }
        match self.top {
            Some(top) if seq <= top => {
                let age = top - seq;
                if age >= self.window {
                    return Err(2); // TooOld
                }
                if self.seen.contains(&seq) {
                    return Err(1); // Replayed
                }
                self.seen.insert(seq);
            }
            _ => {
                // seq > top (ou premiere trame) : la fenetre avance,
                // les seqs sortis de la fenetre sont evinces.
                if self.window > 0 {
                    self.seen.retain(|&s| seq - s < self.window);
                } else {
                    self.seen.clear();
                }
                self.seen.insert(seq);
                self.top = Some(seq);
            }
        }
        if self.ids.insert(id) {
            self.order.push_back(id);
            if self.order.len() > self.dedup_cap {
                if let Some(old) = self.order.pop_front() {
                    self.ids.remove(&old);
                }
            }
        }
        Ok(())
    }
}

/// Discriminant du resultat d'`admit` — les variantes metier
/// attendues sont mappees ; toute autre erreur est impossible ici
/// et vaut divergence immediate.
fn outcome(r: Result<(), MessagingError>) -> Result<(), u8> {
    match r {
        Ok(()) => Ok(()),
        Err(MessagingError::Replayed) => Err(1),
        Err(MessagingError::TooOld) => Err(2),
        Err(MessagingError::DuplicateId) => Err(3),
        Err(_) => panic!("admit a rendu une erreur hors contrat"),
    }
}

/// Decode une sequence d'evenements compacte :
///   [0]   recv_window (= b % 96 — couvre le clamp a 63)
///   [1]   dedup_cap   (= b % 24 — petites caps, evictions denses)
///   [2..] evenements : tag puis charge
///     tag%4==0 Admit   : 8 o seq_raw + 1 o id
///     tag%4==1 Resume  : 8 o top
///     tag%4==2 Reset   : -
///     tag%4==3 Admit a seq courte : 1 o id, seq = tag
fn decode_seq(raw: u64) -> u64 {
    match raw % 8 {
        0..=3 => raw % 256,
        4 | 5 => raw % (1 << 40),
        6 => raw,
        _ => BOUNDARIES[((raw >> 8) & 0xff) as usize % BOUNDARIES.len()],
    }
}

fn take<'a>(it: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if it.len() < n {
        return None;
    }
    let (h, t) = it.split_at(n);
    *it = t;
    Some(h)
}

fn take1(it: &mut &[u8]) -> Option<u8> {
    take(it, 1).map(|b| b[0])
}

fn u64_le(b: &[u8]) -> u64 {
    u64::from_le_bytes(b.try_into().unwrap_or([0; 8]))
}

fuzz_target!(|data: &[u8]| {
    let mut it = data;
    let (Some(wb), Some(cb)) = (take1(&mut it), take1(&mut it)) else {
        return;
    };
    let cfg = MessagingConfig {
        recv_window: u32::from(wb % 96),
        dedup_cap: usize::from(cb % 24),
        ..Default::default()
    };
    let mut w = RecvWindow::new(&cfg);
    let mut m = Model::new(&cfg);

    while let Some(tag) = take1(&mut it) {
        match tag % 4 {
            0 => {
                let (Some(s), Some(i)) = (take(&mut it, 8), take1(&mut it)) else {
                    break;
                };
                let seq = decode_seq(u64_le(s));
                let impl_res = outcome(w.admit(seq, &mk_id(i)));
                let model_res = m.admit(seq, i);
                assert_eq!(
                    impl_res, model_res,
                    "divergence admit(seq={seq}, id={i}) : impl={impl_res:?} model={model_res:?}"
                );
                assert_eq!(w.seen_id(&mk_id(i)), m.ids.contains(&i));
            }
            1 => {
                let Some(t) = take(&mut it, 8) else { break };
                let top = u64_le(t);
                w = RecvWindow::resume(&cfg, top);
                m.resume(&cfg, top);
            }
            2 => {
                w = RecvWindow::new(&cfg);
                m = Model::new(&cfg);
            }
            _ => {
                let Some(i) = take1(&mut it) else { break };
                let seq = u64::from(tag);
                let impl_res = outcome(w.admit(seq, &mk_id(i)));
                let model_res = m.admit(seq, i);
                assert_eq!(impl_res, model_res);
                assert_eq!(w.seen_id(&mk_id(i)), m.ids.contains(&i));
            }
        }
        assert_eq!(w.top(), m.top, "top diverge");
    }
});
