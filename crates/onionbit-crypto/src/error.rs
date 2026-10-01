// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs typées de `onionbit-crypto`.

use thiserror::Error;

/// Erreur de cryptographie.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Cle publique/cle privee mal formee.
    #[error("cle mal formee: {0}")]
    BadKey(String),
    /// Signature invalide.
    #[error("signature invalide")]
    InvalidSignature,
    /// Erreur de derivation de cles (DH/HKDF).
    #[error("echec de derivation de cles: {0}")]
    KeyDerivation(String),
    /// Erreur de chiffrement/dechiffrement AEAD.
    #[error("echec de chiffrement/dechiffrement")]
    Aead,
    /// Message trop court pour contenir les donnees attendues.
    #[error("message trop court (attendu au moins {expected} octets, recu {actual})")]
    Truncated { expected: usize, actual: usize },
}
