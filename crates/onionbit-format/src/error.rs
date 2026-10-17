// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs typées de `onionbit-format`.

use thiserror::Error;

/// Erreur de parsing/serialisation d'un format Tribler.
#[derive(Debug, Error)]
pub enum FormatError {
    /// Entree tronquee (fin de donnees atteinte prematurement).
    #[error("entree tronquee a l'offset {offset}")]
    Truncated { offset: usize },
    /// Bencode invalide.
    #[error("bencode invalide a l'offset {offset}: {reason}")]
    BadBencode { offset: usize, reason: String },
    /// Profondeur d'imbrication maximale depassee (protection DoS).
    #[error("profondeur d'imbrication bencode depassee (max {max})")]
    DepthExceeded { max: usize },
    /// Taille maximale depassee (protection DoS).
    #[error("taille maximale depassee ({actual} > {max} octets)")]
    SizeExceeded { actual: usize, max: usize },
    /// Champ obligatoire absent du .torrent.
    #[error("champ obligatoire absent du torrent: {0}")]
    MissingField(&'static str),
    /// Lien magnet invalide.
    #[error("lien magnet invalide: {0}")]
    BadMagnet(String),
    /// Adresse de service cache Tor v3 invalide (ADR-0028).
    #[error("adresse onion invalide: {0}")]
    BadOnionAddr(String),
    /// Adresse multi-plan d'un pair invalide (ADR-0029).
    #[error("adresse de pair invalide: {0}")]
    BadPeerAddr(String),
}

/// Alias de `Result` pour ce crate.
pub type Result<T> = core::result::Result<T, FormatError>;
