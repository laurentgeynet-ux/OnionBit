// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entrees hostiles contre le parseur stealth (ADR-0017, etape 50) :
//! `hs1_open` (cote pont, filtre X' partage), `hs2_open` (cote
//! client) et `open_frame` (sessions etablies des deux sens). Toute
//! entree doit produire `Err(Reject)` ou `Ok` borne — jamais de
//! panic, jamais d'allocation non bornee.
//!
//! Le premier octet choisit la surface attaquee ; les suivants sont
//! le datagramme brut.

#![no_main]

use std::sync::{Mutex, OnceLock};

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_crypto::stealth::HiddenEph;
use onionbit_ipv8::stealth::{
    hs1_open, hs1_seal, hs2_open, hs2_seal, Hs1ClientCtx, StealthParams, StealthSession,
    XPrimeFilter,
};

struct Etat {
    bridge_sk: [u8; 32],
    bridge_pk: [u8; 32],
    params: StealthParams,
    filt: XPrimeFilter,
    x: HiddenEph,
    ctx: Hs1ClientCtx,
    sess_bridge: StealthSession,
    sess_client: StealthSession,
}

fn etat() -> &'static Mutex<Etat> {
    static ETAT: OnceLock<Mutex<Etat>> = OnceLock::new();
    ETAT.get_or_init(|| {
        let params = StealthParams::default();
        let bridge = LibNaClSecretKey::generate();
        let bridge_pk = *bridge.public_key().crypt_x25519().as_bytes();
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &bridge_pk, b"seed", 1_760_000_000, &params).unwrap();
        let mut filt = XPrimeFilter::new(params.hs_timestamp_skew_secs, params.xprime_set_max);
        let acc = hs1_open(
            bridge.crypt_x25519().as_bytes(),
            &bridge_pk,
            &d1,
            &mut filt,
            1_760_000_000,
            &params,
        )
        .unwrap();
        let y = HiddenEph::generate();
        let (d2, sess_bridge) = hs2_seal(&y, &acc, 1_760_000_000, &params);
        let sess_client = hs2_open(&x, &ctx, &d2, 1_760_000_000, &params).unwrap();
        Mutex::new(Etat {
            bridge_sk: *bridge.crypt_x25519().as_bytes(),
            bridge_pk,
            params,
            filt,
            x,
            ctx,
            sess_bridge,
            sess_client,
        })
    })
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // `&mut *e` : emprunts disjoints des champs malgre le guard.
    let e = &mut *etat().lock().unwrap_or_else(|p| p.into_inner());
    let dgram = &data[1..];
    let params = e.params.clone();
    match data[0] % 4 {
        0 => {
            // hs1 vu par le pont (filtre X' reel).
            let _ = hs1_open(
                &e.bridge_sk,
                &e.bridge_pk,
                dgram,
                &mut e.filt,
                1_760_000_000,
                &params,
            );
        }
        1 => {
            let _ = e.sess_bridge.open_frame(dgram);
        }
        2 => {
            let _ = e.sess_client.open_frame(dgram);
        }
        _ => {
            let _ = hs2_open(&e.x, &e.ctx, dgram, 1_760_000_000, &e.params);
        }
    }
});
