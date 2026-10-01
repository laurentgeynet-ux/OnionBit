// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cellule tunnel : parse, flags, swap de circuit_id et crypto de
//! couche — y compris les retrecissements post-decrypt (une cellule
//! dechiffrant a 29 octets ne doit pas paniquer `check_cell_flags`).

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::session::Direction;
use onionbit_tunnel::cell::{self, Cell};

fuzz_target!(|data: &[u8]| {
    let _ = Cell::parse(data);
    let _ = cell::check_cell_flags(data, 8);
    let _ = cell::check_cell_flags(data, 0);
    let _ = cell::decrypt_cell(data, Direction::Forward, &[]);
    let _ = cell::decrypt_cell(data, Direction::Backward, &[]);
    let mut no_keys = [];
    let _ = cell::encrypt_cell(data, Direction::Forward, &mut no_keys);
    let _ = Cell::swap_circuit_id(data, 0xDEAD);
    if let Ok(c) = Cell::parse(data) {
        let _ = c.unwrap(&onionbit_ipv8::packet::prefix_of(
            &onionbit_tunnel::TUNNEL_COMMUNITY_ID,
        ));
    }
});
