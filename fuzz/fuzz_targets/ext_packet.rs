// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Communaute d'extension OnionBit-only (ADR-0015) : tout pair peut
//! envoyer un datagramme arbitraire sur le prefixe dedie.
//! `Packet::parse` (politique `WIRE_EXT` — tout signe, pas de dist)
//! puis `Hello::unpack` ne doivent jamais paniquer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_ipv8::ext::{EXT_COMMUNITY_ID, WIRE_EXT};
use onionbit_ipv8::ext::Hello;
use onionbit_ipv8::packet::Packet;
use onionbit_ipv8::serializer::Reader;

fuzz_target!(|data: &[u8]| {
    if let Ok(pkt) = Packet::parse(data, Some(&EXT_COMMUNITY_ID), &WIRE_EXT) {
        let mut r = Reader::new(&pkt.payload);
        let _ = Hello::unpack(&mut r);
    }
});
