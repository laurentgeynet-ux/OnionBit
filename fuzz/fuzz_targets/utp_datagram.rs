// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Datagrammes uTP : surface de sortie — des pairs BitTorrent
//! arbitraires envoient des datagrammes reencapsules puis parses par
//! `UtpHeader::deserialize` (extensions, selective-ack, tailles).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = librqbit_utp::raw::UtpHeader::deserialize(data);
});
