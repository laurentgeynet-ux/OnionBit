// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fichier `OBD` hostile : l'entree est ecrite telle quelle comme
//! `.obd` puis toutes les surfaces de parsing sont exercees —
//! `open_scan`, `open`, `read_range` a des offsets arbitraires,
//! `set_len`/`write_range` sur un fichier eventuellement accepte.
//! Invariant : jamais de panic, quelle que soit la corruption
//! (troncature, bit-flip, longueurs forgees dans les sceaux).

#![no_main]

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::obdfile::{ObdFile, PrivateStoreKeys};

fuzz_target!(|data: &[u8]| {
    // Borne le cout disque : chaque input produit un fichier reel.
    if data.len() > (1 << 20) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("obd-fuzz-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("input.obd");
    if std::fs::write(&path, data).is_err() {
        return;
    }
    let keys = PrivateStoreKeys::from_root(&[42u8; 32]);
    let _ = ObdFile::scan_path(&path, &keys);
    if let Ok(f) = ObdFile::open(&path, &keys.file_cipher(&[0u8; 20], b"x")) {
        let mut buf = vec![0u8; 8192];
        let _ = f.read_range(0, &mut buf);
        let _ = f.read_range(u64::MAX - 16, &mut buf[..64]);
        let _ = f.read_range(4097, &mut buf[..100]);
        let _ = f.set_len(data.len() as u64 % 65536);
        let n = data.len().min(4096);
        let _ = f.write_range(0, &data[..n]);
    }
    let _ = std::fs::remove_file(&path);
});
