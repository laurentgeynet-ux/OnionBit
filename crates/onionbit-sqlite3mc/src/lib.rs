// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shim de liaison `sqlite3` (ADR-0031) : le `build.rs` de ce crate
//! compile l'amalgamation SQLite3 Multiple Ciphers en archive
//! statique `sqlite3`, que `libsqlite3-sys` (mode non-`bundled`)
//! lie a la place du SQLite vanilla.
//!
//! Ce crate ne contient aucune logique metier : il garantit
//! uniquement la presence de la bibliotheque dans le graphe de
//! dependances. Le chiffrement se pilote en SQL (`PRAGMA cipher`,
//! `PRAGMA key`) via `rusqlite`.

/// Version SQLite3 Multiple Ciphers compilee (ex. `"2.5.1"`).
pub const SQLITE3MC_VERSION: &str = sqlite3mc_src::SQLITE3MC_VERSION;

/// Version SQLite embarquee dans l'amalgamation (ex. `"3.53.4"`).
pub const SQLITE_VERSION: &str = sqlite3mc_src::SQLITE_VERSION;
