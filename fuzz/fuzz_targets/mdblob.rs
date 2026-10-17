// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Catalogue `mdblob` (ADR-0025, servi aussi en `.onion` depuis
// l'etape 126) — le corps arrive LZ4-frame puis une liste
// d'entrees `MetadataEntry` signees. Surface hostile : le createur
// (ou un tiers qui forge des entrees) controle entierement le
// corps. Invariant : decode LZ4 borne, parse borne, verification
// de signature jamais paniquee, borne memoire.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_format::mdblob::{parse_blob, read_entry};
use std::io::Read;

/// Borne de decompression du banc — le fuzzer doit pouvoir prouver
/// qu'un « zip bomb » LZ4 ne peut pas deborder la memoire.
const MAX_DECOMPRESSED: usize = 4 << 20;

fuzz_target!(|data: &[u8]| {
    // 1) corps brut en entrees mdblob (plan IPv8 SelectResponse)
    if let Ok(entries) = parse_blob(data) {
        for e in &entries {
            if let Some(h) = e.header() {
                let _ = h.verify_signature();
            }
        }
    }
    // 2) lecture d'entree a offset pile par l'entree
    if data.len() >= 2 {
        let off = (u16::from_be_bytes([data[0], data[1]]) as usize) % data.len();
        if let Ok((entry, _consumed)) = read_entry(data, off) {
            if let Some(h) = entry.header() {
                let _ = h.verify_signature();
            }
        }
    }
    // 3) le chemin `.onion` : LZ4 frame → blob → entrees
    let mut dec = lz4_flex::frame::FrameDecoder::new(data);
    let mut plain = Vec::new();
    if dec
        .by_ref()
        .take(MAX_DECOMPRESSED as u64)
        .read_to_end(&mut plain)
        .is_ok()
    {
        if let Ok(entries) = parse_blob(&plain) {
            for e in &entries {
                if let Some(h) = e.header() {
                    let _ = h.verify_signature();
                }
            }
        }
    }
});
