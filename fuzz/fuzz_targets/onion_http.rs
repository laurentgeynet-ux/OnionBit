// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Parseurs de requetes HTTP des services `.onion` (etapes 126/127)
// — la zone en-tete (<= 8 Kio par `read_request_head`) vient d'un
// client Tor hostile. Invariant : classification stricte
// (400/404/405 ou requete valide), jamais de panic, champs bornes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_core::services::{onion_channels, onion_vault};

fuzz_target!(|data: &[u8]| {
    let head = String::from_utf8_lossy(data);
    if let Ok(q) = onion_channels::parse_channel_request(&head) {
        // Une requete acceptee reste exploitable : borne le canal.
        assert!(q.channel_pk.len() == 64, "channel_pk non canonique");
    }
    let _ = onion_vault::parse_vault_request(&head);
});
