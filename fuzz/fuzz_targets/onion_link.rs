// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Lien messagerie `.onion` (ADR-0029 etape 124) — le preambule
// `OBMS` et les trames prefixees `u32` BE sont ecrits par un pair
// hostile au bout d'un circuit Tor. Invariant : la borne est lue
// AVANT l'allocation (`read_frame` refuse `len > max_len` sans
// allouer), le preambule est de forme fixe, jamais de panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_messaging::onion_link::{decode_preamble, read_frame, PREAMBLE_LEN};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime fuzz")
    })
}

fuzz_target!(|data: &[u8]| {
    // Preambule : tronquer/padder l'entree a la forme fixe.
    let _ = decode_preamble(data);
    if data.len() > PREAMBLE_LEN {
        let _ = decode_preamble(&data[..PREAMBLE_LEN]);
    }
    // Trames prefixees en serie : lire jusqu'a epuisement du flux
    // ou 8 trames (borne de boucle du banc — pas de spin infini).
    let mut slice = data;
    rt().block_on(async {
        for _ in 0..8 {
            match read_frame(&mut slice, 1 << 20).await {
                Ok(Some(_body)) => continue,
                _ => break,
            }
        }
    });
});
