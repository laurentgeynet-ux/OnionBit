// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dispatch des paquets tunnel non signes (`ezr_pack(sig=False)`) :
//! `prefix + msg_id + corps` arrive a nu sur la socket — la whitelist
//! `on_packet_from_circuit` (msg 13/14/17/18) et les parsers de corps
//! ne doivent jamais paniquer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_ipv8::packet::{prefix_of, Packet, WIRE_DEFAULT};
use onionbit_ipv8::serializer::Reader;
use onionbit_tunnel::payload::{self as tp, msg, Cellable};
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

fuzz_target!(|input: (u8, &[u8])| {
    let (msg_id, body) = input;
    let prefix = prefix_of(&TUNNEL_COMMUNITY_ID);
    let mut pkt = Vec::with_capacity(prefix.len() + 1 + body.len());
    pkt.extend_from_slice(&prefix);
    pkt.push(msg_id);
    pkt.extend_from_slice(body);

    // Enveloppe signee (echoue -> le dispatcher retombe sur le
    // dispatch non signe, replique ici).
    let _ = Packet::parse(&pkt, Some(&TUNNEL_COMMUNITY_ID), &WIRE_DEFAULT);
    let mut r = Reader::new(body);
    match msg_id {
        msg::CREATE_E2E => {
            let _ = tp::CreateE2E::unpack(&mut r);
        }
        msg::CREATED_E2E => {
            let _ = tp::CreatedE2E::unpack(&mut r);
        }
        msg::PEERS_REQUEST => {
            let _ = tp::PeersRequest::unpack(&mut r);
        }
        msg::PEERS_RESPONSE => {
            let _ = tp::PeersResponse::unpack(&mut r);
        }
        _ => {}
    }
});
