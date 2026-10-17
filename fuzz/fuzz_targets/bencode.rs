// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Parseur bencode (onionbit-format) — surface hostile premiere :
// chaque trame, payload, `.torrent`, `mdblob` et reponse de tracker
// passe ici. Invariant : jamais de panic, jamais de recursion non
// bornee, jamais d'allocation disproportionnee sur une entree
// arbitraire. On exerce aussi `decode_at` avec des offsets et
// profondeurs issus de l'entree, puis le re-encodage (round-trip).

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_format::bencode::{decode, parser::decode_at};

fuzz_target!(|data: &[u8]| {
    if let Ok(v) = decode(data) {
        // Les accesseurs ne paniquent jamais sur une forme quelconque.
        let _ = v.as_int();
        let _ = v.as_bytes();
        let _ = v.as_str();
        let _ = v.as_list();
        let _ = v.as_dict();
        let _ = v.get(b"info");
        let _ = v.get_str("announce");
        // Re-encodage borne — le encode() d'une valeur decodee reste
        // dans l'ordre de grandeur de l'entree.
        let out = v.encode();
        assert!(
            out.len() <= data.len() + 4096,
            "re-encodage disproportionne"
        );
    }
    // `decode_at` avec offset/profondeur pilotes par l'entree.
    if data.len() >= 2 {
        let off = data[0] as usize % data.len().max(1);
        let depth = data[1] as usize % 64;
        let _ = decode_at(data, off, depth);
    }
});
