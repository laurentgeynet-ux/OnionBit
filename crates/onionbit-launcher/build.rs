// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `build.rs` de `onionbit-launcher` : embarque `resources.rc`
//! (icone `onionbit.ico` partagee avec `onionbit-daemon`) dans l'exe
//! Windows — les trois copies a la racine du bundle heritent de
//! l'icone OnionBit dans l'Explorateur. Hors Windows : no-op.

fn main() {
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        match embed_resource::compile("resources.rc", embed_resource::NONE) {
            embed_resource::CompilationResult::Ok
            | embed_resource::CompilationResult::NotWindows => {}
            other => println!("cargo:warning=resources.rc non embarquée : {other}"),
        }
    }
}

#[cfg(not(windows))]
fn embed_icon() {}
