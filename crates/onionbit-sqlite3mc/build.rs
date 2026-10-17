// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compile l'amalgamation SQLite3 Multiple Ciphers en archive
//! statique `sqlite3` (ADR-0031, spike 136). Sans la feature
//! `bundled`, `libsqlite3-sys` emet `rustc-link-lib=sqlite3` : le
//! nom d'archive choisi ici satisfait cette reference et le codec
//! remplace le SQLite vanilla.
//!
//! Le codec s'auto-initialise dans `sqlite3_initialize` —
//! `SQLITE_EXTRA_INIT` n'est pas requis. Tout le pilotage se fait
//! en SQL (`PRAGMA cipher`, `PRAGMA key`) via rusqlite.

use std::env;

fn main() {
    let source_dir = sqlite3mc_src::source_dir();

    let mut build = cc::Build::new();
    build
        .file(source_dir.join(sqlite3mc_src::SOURCE_FILE))
        .include(source_dir)
        // Repli exact des flags `bundled` de libsqlite3-sys 0.38 :
        // le remplacement doit exposer le meme jeu de features.
        .flag("-DSQLITE_CORE")
        .flag("-DSQLITE_DEFAULT_FOREIGN_KEYS=1")
        .flag("-DSQLITE_ENABLE_API_ARMOR")
        .flag("-DSQLITE_ENABLE_COLUMN_METADATA")
        .flag("-DSQLITE_ENABLE_DBSTAT_VTAB")
        .flag("-DSQLITE_ENABLE_FTS3")
        .flag("-DSQLITE_ENABLE_FTS3_PARENTHESIS")
        .flag("-DSQLITE_ENABLE_FTS5")
        .flag("-DSQLITE_ENABLE_JSON1")
        .flag("-DSQLITE_ENABLE_LOAD_EXTENSION=1")
        .flag("-DSQLITE_ENABLE_MEMORY_MANAGEMENT")
        .flag("-DSQLITE_ENABLE_RTREE")
        .flag("-DSQLITE_ENABLE_STAT4")
        .flag("-DSQLITE_SOUNDEX")
        .flag("-DSQLITE_THREADSAFE=1")
        .flag("-DSQLITE_USE_URI")
        .flag("-DHAVE_USLEEP=1")
        .flag("-DHAVE_ISNAN")
        .flag("-D_POSIX_THREAD_SAFE_FUNCTIONS")
        // ADR-0031 §2 : le codec ne couvre pas les fichiers
        // temporaires SQLite (spill de tris, tables temp) — forcer
        // la memoire a la compilation, en plus du pragma runtime.
        .flag("-DSQLITE_TEMP_STORE=3")
        // Surface minimale : seul le cipher ChaCha20 est compile ;
        // aucune base legacy AES/RC4/sqlcipher a lire.
        .flag("-DHAVE_CIPHER_AES_128_CBC=0")
        .flag("-DHAVE_CIPHER_AES_256_CBC=0")
        .flag("-DHAVE_CIPHER_SQLCIPHER=0")
        .flag("-DHAVE_CIPHER_RC4=0")
        .flag("-DHAVE_CIPHER_ASCON128=0")
        .flag("-DHAVE_CIPHER_AEGIS=0")
        .warnings(false);

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        build.flag("-DHAVE_LOCALTIME_R");
    }
    if env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|v| v.split(',').any(|tf| tf == "crt-static"))
        && env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc")
    {
        build.static_crt(true);
    }
    if let Ok(limit) = env::var("SQLITE_MAX_VARIABLE_NUMBER") {
        build.flag(format!("-DSQLITE_MAX_VARIABLE_NUMBER={limit}"));
    }
    println!("cargo:rerun-if-env-changed=SQLITE_MAX_VARIABLE_NUMBER");

    // `cc::compile` emet a la fois `rustc-link-search=native` et
    // `rustc-link-lib=static=sqlite3` — pas de println! requis.
    build.compile("sqlite3");
}
