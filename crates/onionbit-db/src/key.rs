// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cle de chiffrement de la base locale (ADR-0031).
//!
//! `DbKey` enrobe les 32 octets bruts du codec sqlite3mc `chacha20`
//! avec `zeroize` au drop — la cle ne vit qu'en memoire, jamais
//! serialisee ni loggee. La derivation HKDF `onionbit/db-key/v1`
//! depuis `store_root` est faite cote `onionbit-crypto` / core ;
//! ce module ne fait que le portage + la presentation au pragma.

use zeroize::{Zeroize, ZeroizeOnDrop};

/// Cle brute 256 bits du codec sqlite3mc `chacha20` (ADR-0031).
///
/// `Copy`/`Clone`/`Debug` volontairement absents : une seule copie
/// existe — drop la zeroize — et la cle n'apparait jamais dans les
/// logs. La serialisation `PRAGMA key` passe par [`KeyPragma`] dont
/// le buffer fixe est zeroize pareil (pas de `String` intermediaire
/// laissant l'hex de la cle sur le heap).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct DbKey([u8; 32]);

impl DbKey {
    /// Enveloppe une cle deja derivee (`derive_db_key_seed` cote
    /// `onionbit-crypto` — HKDF `onionbit/db-key/v1` sur
    /// `store_root`).
    pub fn new(raw: [u8; 32]) -> Self {
        Self(raw)
    }

    /// Construit le `PRAGMA key` dans un buffer fixe zeroize au
    /// drop — `x'<64 hex>'` : le codec prend les octets bruts sans
    /// passer par son KDF interne (notre HKDF est le seul niveau de
    /// derivation, ADR-0031 §2).
    pub(crate) fn pragma(&self) -> KeyPragma {
        let mut buf = [0u8; 80];
        buf[..14].copy_from_slice(b"PRAGMA key=\"x'");
        for (i, b) in self.0.iter().enumerate() {
            buf[14 + 2 * i] = b"0123456789abcdef"[(b >> 4) as usize];
            buf[15 + 2 * i] = b"0123456789abcdef"[(b & 0xf) as usize];
        }
        buf[78] = b'\'';
        buf[79] = b'"';
        KeyPragma(buf)
    }
}

/// Tampon du `PRAGMA key` — ASCII fixe, zeroize au drop.
pub(crate) struct KeyPragma([u8; 80]);

impl KeyPragma {
    pub(crate) fn as_str(&self) -> &str {
        // Buffer construit ici : uniquement ASCII par construction.
        std::str::from_utf8(&self.0).expect("pragma key ascii")
    }
}

impl Drop for KeyPragma {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
