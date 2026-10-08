// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs du crate `onionbit-ipv8`.

use thiserror::Error;

/// Erreur de (de)serialisation ou de traitement de paquet IPv8.
#[derive(Debug, Error)]
pub enum Ipv8Error {
    /// Buffer trop court pour le champ demande.
    #[error("paquet tronque : besoin de {need} octets, {have} restants")]
    Truncated { need: usize, have: usize },
    /// Contenu mal forme.
    #[error("paquet mal forme : {0}")]
    Malformed(&'static str),
    /// Signature Ed25519 invalide.
    #[error("signature invalide")]
    InvalidSignature,
    /// Message inconnu pour cette community.
    #[error("message inconnu : id {0}")]
    UnknownMessage(u8),
    /// Ressource pas encore prete (ex. circuit en construction) —
    /// distinct de `Malformed` : ce n'est pas une erreur de wire, le
    /// demandeur peut retenter plus tard.
    #[error("ressource non prete : {0}")]
    NotReady(&'static str),
    /// Erreur crypto sous-jacente.
    #[error(transparent)]
    Crypto(#[from] onionbit_crypto::CryptoError),
    /// Erreur reseau.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
