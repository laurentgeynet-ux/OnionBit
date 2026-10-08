// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Communaute d'extension OnionBit-only (ADR-0015) : tout pair peut
//! envoyer un datagramme arbitraire sur le prefixe dedie.
//! `Packet::parse` (politique `WIRE_EXT` — tout signe, pas de dist)
//! puis `Hello::unpack`/`Attestation::unpack`/`LedgerLink::unpack`
//! ne doivent jamais paniquer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::ext::{
    ledger::LedgerLink, obf, Attestation, Hello, EXT_COMMUNITY_ID, WIRE_EXT,
};
use onionbit_ipv8::packet::Packet;
use onionbit_ipv8::serializer::Reader;

fuzz_target!(|data: &[u8]| {
    if let Ok(pkt) = Packet::parse(data, Some(&EXT_COMMUNITY_ID), &WIRE_EXT) {
        let mut r = Reader::new(&pkt.payload);
        let _ = Hello::unpack(&mut r);
        let mut r = Reader::new(&pkt.payload);
        let _ = Attestation::unpack(&mut r);
        // Liens du ledger bilateral (Phase 9c) : les deux formes
        // (proposition `sig_b` vide et lien scelle) sont bornees.
        let mut r = Reader::new(&pkt.payload);
        let _ = LedgerLink::unpack(&mut r, false);
        let mut r = Reader::new(&pkt.payload);
        let _ = LedgerLink::unpack(&mut r, true);
    }
    // Enveloppe OBF (Phase 9e) : aller-retour avec le payload fuzzé
    // (seal produit une trame correcte quel que soit le contenu,
    // open doit la relire sans panic) + ouverture directe d'octets
    // arbitraires (chemin de reception — AEAD refuse, jamais panic).
    let a = LibNaClSecretKey::generate();
    let b = LibNaClSecretKey::generate();
    let bucket = if data.len() > 1 { (data[0] as usize) | 16 } else { 16 };
    if let Ok(env) = obf::seal(&b.public_key(), &a, data.first().copied().unwrap_or(0), data, bucket)
    {
        let _ = obf::open(&a.public_key(), &b, &env);
    }
    let _ = obf::open(&a.public_key(), &b, data);
});
