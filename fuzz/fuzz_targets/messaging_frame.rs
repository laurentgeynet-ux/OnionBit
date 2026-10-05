// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Trame messagerie (ADR-0011) : `Frame::open` est la surface de
// parsing hostile par excellence — une trame recue sur un circuit
// e2e est entierement controlee par le pair. Le parseur borne la
// taille avant tout travail (`max_frame_len`), puis le bencode
// borne, la forme de cles et la signature — invariant : jamais de
// panic, jamais d'allocation infinie.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use onionbit_messaging::{Frame, MessagingConfig};
use std::sync::OnceLock;

/// Cle de contact fixe — la signature rejettera la plupart des
/// entrees arbitraires, mais le chemin codec (borne, bencode, forme
/// de cles, tailles de champs) est integralement exerce.
fn peer_pk() -> &'static LibNaClPublicKey {
    static PK: OnceLock<LibNaClPublicKey> = OnceLock::new();
    PK.get_or_init(|| LibNaClSecretKey::generate().public_key())
}

fuzz_target!(|data: &[u8]| {
    let cfg = MessagingConfig::default();
    let key = [0u8; 32];
    let _ = Frame::open(data, peer_pk(), &key, &cfg);
});
