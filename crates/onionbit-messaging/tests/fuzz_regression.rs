// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Harnais de robustesse du parseur de trames messagerie (ADR-0011,
//! « codec hostile » — `MS-3`/`MS-10`) : `Frame::open` ne doit
//! JAMAIS paniquer ni allouer sans borne, quelle que soit l'entree.
//!
//! Miroir stable de la cible `messaging_frame` du harnais
//! cargo-fuzz (`fuzz/`), executable en CI sans nightly — meme
//! reglage que `crates/onionbit-tunnel/tests/fuzz_regression.rs`.

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_messaging::frame::MSG_ID_LEN;
use onionbit_messaging::{Frame, MessagingConfig, MessagingError, RecvWindow};
use proptest::prelude::*;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;

/// Cle de test fixe : l'objectif est la robustesse du parseur, la
/// verification de signature n'est pas ce qu'on fuzz (une entree
/// arbitraire echoue a la signature apres codec — c'est le codec
/// qui est la surface).
fn peer_pk() -> &'static onionbit_crypto::ipv8::keys::LibNaClPublicKey {
    static PK: OnceLock<onionbit_crypto::ipv8::keys::LibNaClPublicKey> = OnceLock::new();
    PK.get_or_init(|| LibNaClSecretKey::generate().public_key())
}

/// Passe `data` dans le parseur de trame — invariant : `Ok`/`Err`,
/// jamais de panic. Une cle bidon convient : le chemin codec
/// (borne + bencode + forme) est exerce avant tout refus crypto.
fn exercise_frame(data: &[u8]) {
    let cfg = MessagingConfig::default();
    let key = [0u8; 32];
    let _ = Frame::open(data, peer_pk(), &key, &cfg);
}

/// Frontieres deterministes du format : autour de la borne
/// `max_frame_len`, des entiers bencode, des cles de dict.
#[test]
fn frontieres_trame_ne_paniquent_pas() {
    for len in [
        0usize, 1, 2, 3, 4, 5, 10, 30, 60, 63, 64, 65, 255, 256, 1024, 4095, 4096,
    ] {
        exercise_frame(&vec![0u8; len]);
        exercise_frame(&vec![0xFF; len]);
        exercise_frame(&vec![b'd'; len]);
        exercise_frame(&vec![b'e'; len]);
    }
    // Troncatures d'une trame valide : chaque prefixe doit etre
    // refuse proprement.
    let sk = LibNaClSecretKey::generate();
    let cfg = MessagingConfig::default();
    let key = [1u8; 32];
    let f = Frame::new(onionbit_messaging::MsgKind::Msg, 1, 1, b"corps".to_vec());
    let wire = f.seal(&sk, &key, &cfg).unwrap();
    for i in 0..wire.len() {
        exercise_frame(&wire[..i]);
    }
    // Dict bencode avec mega-compteurs de champs et cles geantes.
    exercise_frame(b"d99999:aaae");
    exercise_frame(b"d1:ai2147483647ee");
    exercise_frame(b"le");
    exercise_frame(b"d1:ali1ei2elieee");
}

/// Rejoue le corpus minimise de la campagne `messaging_frame`
/// (merge libFuzzer : chaque input couvre des features que les
/// autres n'atteignent pas). Versionne sous `tests/fuzz_corpus/` —
/// aucun de ces inputs ne doit jamais paniquer.
#[test]
fn corpus_campagne_ne_panique_pas() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_corpus");
    let mut n = 0usize;
    for entry in std::fs::read_dir(&dir).expect("corpus de campagne lisible") {
        let path = entry.expect("entree corpus lisible").path();
        if path.is_file() {
            exercise_frame(&std::fs::read(&path).expect("input corpus lisible"));
            n += 1;
        }
    }
    assert!(n > 0, "corpus de campagne vide");
}

// ---------------------------------------------------------------------
// Machine d'etat anti-rejeu — miroir stable de la cible cargo-fuzz
// `messaging_window` : meme modele de reference, memes invariants.
// ---------------------------------------------------------------------

/// Modele de reference : ensemble borne de seqs vus + cache FIFO
/// d'`id`, sans bitmap — la spec naive contre laquelle `admit` est
/// comparee evenement par evenement.
struct WindowModel {
    top: Option<u64>,
    seen: HashSet<u64>,
    ids: HashSet<u8>,
    order: VecDeque<u8>,
    window: u64,
    dedup_cap: usize,
}

impl WindowModel {
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
            return Err(3);
        }
        match self.top {
            Some(top) if seq <= top => {
                let age = top - seq;
                if age >= self.window {
                    return Err(2);
                }
                if self.seen.contains(&seq) {
                    return Err(1);
                }
                self.seen.insert(seq);
            }
            _ => {
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

/// Discriminant du resultat d'`admit` (1=Replayed, 2=TooOld,
/// 3=DuplicateId) — toute autre variante est hors contrat.
fn admit_outcome(r: Result<(), MessagingError>) -> Result<(), u8> {
    match r {
        Ok(()) => Ok(()),
        Err(MessagingError::Replayed) => Err(1),
        Err(MessagingError::TooOld) => Err(2),
        Err(MessagingError::DuplicateId) => Err(3),
        Err(e) => panic!("admit a rendu une erreur hors contrat : {e}"),
    }
}

/// Operations de la machine d'etat fuzzee.
#[derive(Debug, Clone)]
enum WindowOp {
    Admit { seq: u64, id: u8 },
    Resume { top: u64 },
    Reset,
}

/// `seq` arbres de proptest : melange dense/petit, moyen et
/// frontieres explicites (dont > u32::MAX et u64::MAX).
fn seq_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => 0u64..256,
        2 => 0u64..(1 << 40),
        1 => any::<u64>(),
        3 => prop::sample::select(vec![
            0u64,
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
        ]),
    ]
}

fn op_strategy() -> impl Strategy<Value = WindowOp> {
    prop_oneof![
        7 => (seq_strategy(), any::<u8>()).prop_map(|(seq, id)| WindowOp::Admit { seq, id }),
        1 => seq_strategy().prop_map(|top| WindowOp::Resume { top }),
        1 => Just(WindowOp::Reset),
    ]
}

proptest! {
    /// Differenciel impl vs modele : pour toute sequence
    /// d'operations et toute config (fenetre/cap dedup bornees),
    /// `admit`, `seen_id` et `top` doivent coincider exactement.
    #[test]
    fn recv_window_equivaut_au_modele(
        recv_window in 0u32..96,
        dedup_cap in 0usize..24,
        ops in proptest::collection::vec(op_strategy(), 0..200),
    ) {
        let cfg = MessagingConfig {
            recv_window,
            dedup_cap,
            ..Default::default()
        };
        let mut w = RecvWindow::new(&cfg);
        let mut m = WindowModel::new(&cfg);
        for op in ops {
            match op {
                WindowOp::Admit { seq, id } => {
                    let id_arr = [id; MSG_ID_LEN];
                    prop_assert_eq!(
                        admit_outcome(w.admit(seq, &id_arr)),
                        m.admit(seq, id),
                        "divergence admit(seq={}, id={})",
                        seq,
                        id
                    );
                    prop_assert_eq!(w.seen_id(&id_arr), m.ids.contains(&id));
                }
                WindowOp::Resume { top } => {
                    w = RecvWindow::resume(&cfg, top);
                    m.resume(&cfg, top);
                }
                WindowOp::Reset => {
                    w = RecvWindow::new(&cfg);
                    m = WindowModel::new(&cfg);
                }
            }
            prop_assert_eq!(w.top(), m.top);
        }
    }
}

proptest! {
    /// Octets arbitraires bornes a la taille de trame max : le
    /// parseur borne avant de parser — au-dela c'est un refus
    /// immediat, en dessous le bencode borne prend le relais.
    #[test]
    fn trame_jamais_de_panic(data in proptest::collection::vec(any::<u8>(), 0..=33000)) {
        exercise_frame(&data);
    }

    /// Mutations autour d'une trame valide : prefixe valide +
    /// queue hostile — exerce la validation d'ensemble de cles.
    #[test]
    fn suffixe_hostile_ne_panique_pas(tail in proptest::collection::vec(any::<u8>(), 0..=512)) {
        let sk = LibNaClSecretKey::generate();
        let cfg = MessagingConfig::default();
        let key = [2u8; 32];
        let f = Frame::new(onionbit_messaging::MsgKind::Msg, 1, 1, b"x".to_vec());
        let mut wire = f.seal(&sk, &key, &cfg).unwrap();
        wire.extend_from_slice(&tail);
        exercise_frame(&wire);
    }
}
