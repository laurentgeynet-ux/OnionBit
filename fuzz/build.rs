// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// Compile le shim de bornes sancov quand la cible est windows-msvc :
// rustc y emet des refs ELF-style `__start_/__stop_` non resolues par
// lld-link. Sur ELF (Linux/WSL) le linker les synthetise — pas de shim.

fn main() {
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:rerun-if-changed=sancov_shim.c");
    if os == "windows" && env == "msvc" {
        let out = std::env::var("OUT_DIR").unwrap();
        cc::Build::new()
            .file("sancov_shim.c")
            .compile("sancov_shim");
        println!("cargo:rustc-link-search=native={out}");
        println!("cargo:rustc-link-lib=static=sancov_shim");
    }
}
