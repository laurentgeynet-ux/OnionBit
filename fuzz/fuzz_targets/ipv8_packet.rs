// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Enveloppe IPv8 : tout pair peut envoyer n'importe quel datagramme
//! signe ou non. `Packet::parse` ne doit jamais paniquer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_ipv8::packet::{Packet, WIRE_DEFAULT};

fuzz_target!(|data: &[u8]| {
    let _ = Packet::parse(data, None, &WIRE_DEFAULT);
    let _ = Packet::parse(data, Some(&onionbit_tunnel::TUNNEL_COMMUNITY_ID), &WIRE_DEFAULT);
});
