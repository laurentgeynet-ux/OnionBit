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
        self.pragma_named("key")
    }

    /// Variante pour `rekey` (migration etape 138 : chiffrement en
    /// place de la copie claire) et `hexrekey` si besoin.
    pub(crate) fn pragma_named(&self, name: &str) -> KeyPragma {
        let mut buf = [0u8; 96];
        let head = format!("PRAGMA {name}=\"x'");
        buf[..head.len()].copy_from_slice(head.as_bytes());
        for (i, b) in self.0.iter().enumerate() {
            let p = head.len() + 2 * i;
            buf[p] = b"0123456789abcdef"[(b >> 4) as usize];
            buf[p + 1] = b"0123456789abcdef"[(b & 0xf) as usize];
        }
        let mut end = head.len() + 64;
        buf[end] = b'\'';
        buf[end + 1] = b'"';
        end += 2;
        KeyPragma { buf, len: end }
    }
}

/// Tampon du `PRAGMA key`/`rekey` — ASCII fixe, zeroize au drop.
/// `len` borne la tranche utile (le reste du buffer reste a zero).
pub(crate) struct KeyPragma {
    buf: [u8; 96],
    len: usize,
}

impl KeyPragma {
    pub(crate) fn as_str(&self) -> &str {
        // Buffer construit ici : uniquement ASCII par construction.
        std::str::from_utf8(&self.buf[..self.len]).expect("pragma key ascii")
    }
}

impl Drop for KeyPragma {
    fn drop(&mut self) {
        self.buf.zeroize();
    }
}
