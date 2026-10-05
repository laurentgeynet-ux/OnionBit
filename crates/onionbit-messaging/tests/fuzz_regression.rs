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
use onionbit_messaging::{Frame, MessagingConfig};
use proptest::prelude::*;
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
