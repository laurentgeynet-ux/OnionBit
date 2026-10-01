// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs du moteur BitTorrent.

/// Resultat des operations de ce crate.
pub type Result<T> = std::result::Result<T, BtError>;

/// Erreurs remontees par l'enveloppe `librqbit`.
#[derive(Debug, thiserror::Error)]
pub enum BtError {
    /// Erreur interne du moteur `librqbit`.
    #[error("moteur bittorrent: {0}")]
    Engine(String),
    /// Le format d'entree est invalide.
    #[error("format: {0}")]
    Format(#[from] onionbit_format::FormatError),
    /// URL ou magnet mal forme.
    #[error("url invalide: {0}")]
    BadUrl(String),
    /// Torrent introuvable dans la session.
    #[error("torrent introuvable: {0}")]
    NotFound(String),
    /// Le torrent ajoute n'a pas produit de handle (ex. list_only).
    #[error("pas de handle de torrent")]
    NoHandle,
    /// Refus impose par une politique reseau (proxy non local,
    /// kill switch engage).
    #[error("politique reseau: {0}")]
    Policy(#[from] onionbit_network_policy::PolicyError),
    /// Erreur d'E/S.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
