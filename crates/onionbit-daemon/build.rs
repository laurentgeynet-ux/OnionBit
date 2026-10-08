// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `build.rs` de `onionbit-daemon` : embarque `resources.rc`
//! (icone `onionbit.ico` + infos de version) dans l'exe Windows.
//! Hors cible Windows : no-op (`CARGO_CFG_WINDOWS` non defini).
//! Un echec de compilation de ressource degrade en warning (l'icone
//! tray a un fallback `Icon::from_rgba`), jamais en erreur de build.

fn main() {
    embed_icon();
}

/// Embarque `resources.rc` dans l'exe. Compile-temps *et* runtime
/// conditions sur Windows : la build-dep `embed-resource` n'est liee
/// que si l'hote de compilation est Windows.
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
