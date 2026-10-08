// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Genere le corpus initial de `fuzz_targets/messaging_frame` dans
//! `fuzz/corpus/messaging_frame/` : trames valides `seal()`ees pour
//! chaque `MsgKind`, tailles de corps aux bornes (`0`, `max_body_len`)
//! et mutations de surface (troncature, octet inverse, prefixe
//! parasite). Le corpus n'est pas versionne (`.gitignore`) — cet
//! utilitaire permet de le regenerer identiquement.
//!
//! Usage : `cargo run -p onionbit-messaging --example fuzz_corpus_seed`

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_messaging::{Frame, MessagingConfig, MsgKind};
use std::path::Path;

fn main() {
    let out = Path::new("fuzz/corpus/messaging_frame");
    std::fs::create_dir_all(out).expect("mkdir corpus");

    let sk = LibNaClSecretKey::generate();
    let cfg = MessagingConfig::default();
    let key = [9u8; 32];

    let mut n = 0usize;
    let mut put = |name: &str, data: &[u8]| {
        std::fs::write(out.join(name), data).expect("write seed");
        n += 1;
    };

    // Trames valides : chaque type, corps courts + corps a la borne.
    let bodies: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"hello".to_vec(),
        vec![0u8; 16],
        vec![0xAB; 512],
        vec![0x42; cfg.max_body_len],
    ];
    for (i, kind) in [
        MsgKind::Hello,
        MsgKind::Accept,
        MsgKind::Reject,
        MsgKind::Msg,
        MsgKind::Ack,
    ]
    .iter()
    .enumerate()
    {
        for (j, body) in bodies.iter().enumerate() {
            let f = Frame::new(*kind, (i * 100 + j) as u64, 1_700_000_000, body.clone());
            let wire = f.seal(&sk, &key, &cfg).expect("seal");
            put(&format!("valid_k{i}_b{j}"), &wire);
        }
    }

    // Seq aux bornes (nonce AEAD : cas limites de compteur).
    for (j, seq) in [0u64, u64::MAX, u64::MAX - 1].iter().enumerate() {
        let f = Frame::new(MsgKind::Msg, *seq, u64::MAX, b"edge".to_vec());
        let wire = f.seal(&sk, &key, &cfg).expect("seal edge");
        put(&format!("seq_{j}"), &wire);
    }

    // Mutations de surface : la couche crypto rejette, le codec et
    // les bornes restent exerces.
    let f = Frame::new(MsgKind::Msg, 7, 1_700_000_000, vec![1u8; 256]);
    let wire = f.seal(&sk, &key, &cfg).expect("seal mut");
    put("trunc_8", &wire[..8]);
    put("trunc_mid", &wire[..wire.len() / 2]);
    put("trunc_last", &wire[..wire.len() - 1]);
    let mut flipped = wire.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xFF;
    put("flip_mid", &flipped);
    let mut garbage = b"not-a-frame".to_vec();
    garbage.extend_from_slice(&wire);
    put("prefix_garbage", &garbage);
    // Dict bencode minimal + valeurs limites vues par le preflight.
    put("bencode_empty_dict", b"de");
    put("bencode_zero", b"i0e");
    put("huge", &vec![0xFFu8; cfg.max_frame_len + 64]);
    put("tiny", b"d");

    println!("{n} seeds -> {}", out.display());
}
