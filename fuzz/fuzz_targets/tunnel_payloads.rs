// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps des payloads tunnel et DHT : chaque `unpack` doit borner
//! ses lectures (`Reader` retourne `Truncated`) — jamais paniquer,
//! jamais allouer sans limite sur des compteurs adversaires.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_ipv8::serializer::Reader;
use onionbit_tunnel::hidden_services::unpack_dht_intro_point;
use onionbit_tunnel::payload::{self as tp, Cellable};

fuzz_target!(|data: &[u8]| {
    let _ = tp::CreateE2E::unpack(&mut Reader::new(data));
    let _ = tp::CreatedE2E::unpack(&mut Reader::new(data));
    let _ = tp::PeersRequest::unpack(&mut Reader::new(data));
    let _ = tp::PeersResponse::unpack(&mut Reader::new(data));
    let _ = tp::LinkE2E::unpack(&mut Reader::new(data));
    let _ = tp::LinkedE2E::unpack(&mut Reader::new(data));
    let _ = tp::EstablishIntro::unpack(&mut Reader::new(data));
    let _ = tp::IntroEstablished::unpack(&mut Reader::new(data));
    let _ = tp::EstablishRendezvous::unpack(&mut Reader::new(data));
    let _ = tp::RendezvousEstablished::unpack(&mut Reader::new(data));
    let _ = tp::RendezvousInfo::unpack_framed(&mut Reader::new(data));
    let _ = tp::Data::unpack(&mut Reader::new(data));
    let _ = tp::Create::unpack(&mut Reader::new(data));
    let _ = tp::Created::unpack(&mut Reader::new(data));
    let _ = tp::Extend::unpack(&mut Reader::new(data));
    let _ = tp::Extended::unpack(&mut Reader::new(data));
    let _ = tp::Destroy::unpack(&mut Reader::new(data));
    let _ = tp::TunnelPing::unpack(&mut Reader::new(data));
    let _ = unpack_dht_intro_point(data);
});
