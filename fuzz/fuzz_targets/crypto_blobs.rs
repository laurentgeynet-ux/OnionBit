// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Blobs chiffres portables (OBM / OBV1) — servis par le coffre
// `.onion` (etape 127) et les ponts pull_store : un pair hostile
// peut repondre n'importe quoi a un pull. Le decodage d'enveloppe
// (magic/version/borne/nonce) precede l'AEAD ; l'AEAD refuse le
// reste. Invariant : rejet propre, jamais de panic, jamais
// d'allocation avant la borne. On exerce aussi les parseurs
// d'adresses multi-plan (`PeerAddress`, `onion_host`) — entree
// utilisateur et contenu de coffre.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::dh::pair_open_in;
use onionbit_crypto::obdfile::PrivateStoreKeys;
use onionbit_crypto::obm;
use onionbit_format::onion::onion_host;
use onionbit_format::peer_addr::PeerAddress;
use std::sync::OnceLock;

fn vault_keys() -> &'static PrivateStoreKeys {
    static K: OnceLock<PrivateStoreKeys> = OnceLock::new();
    K.get_or_init(|| PrivateStoreKeys::from_root(&[0x42; 32]))
}

/// Paire X25519 fixe pour `pair_open_in` (OBV1/`obox` coffre).
fn pair() -> ([u8; 32], [u8; 32]) {
    static P: OnceLock<([u8; 32], [u8; 32])> = OnceLock::new();
    *P.get_or_init(|| {
        let sk = x25519_dalek::StaticSecret::random();
        let pk = x25519_dalek::PublicKey::from(&sk);
        (*pk.as_bytes(), sk.to_bytes())
    })
}

fuzz_target!(|data: &[u8]| {
    // Enveloppe OBM (manifeste prive) — borne a l'ouverture avant
    // tout travail AEAD.
    let _ = obm::open_manifest(vault_keys(), data);
    // Blobs pairbox (OBV1 coffre, obox mailbox) : memes invariants.
    let (pk, sk) = pair();
    let _ = pair_open_in(&pk, &sk, b"onionbit/vault/v1", data);
    let _ = pair_open_in(&pk, &sk, b"onionbit/obox/v1", data);
    // Adresses multi-plan (chaine controlee : coffre importe, saisie).
    let s = String::from_utf8_lossy(data);
    let _ = PeerAddress::parse(&s);
    let _ = onion_host(&s);
});
