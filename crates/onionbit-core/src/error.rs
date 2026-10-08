// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs de la couche domaine/orchestration.

/// Resultat des operations de ce crate.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Erreurs de `onionbit-core`.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Erreur du moteur BitTorrent.
    #[error("bittorrent: {0}")]
    Bt(#[from] onionbit_bittorrent::BtError),
    /// Erreur de persistance.
    #[error("db: {0}")]
    Db(#[from] onionbit_db::DbError),
    /// Erreur de format (magnet/.torrent invalide).
    #[error("format: {0}")]
    Format(#[from] onionbit_format::FormatError),
    /// Erreur d'E/S.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Session deja demarree ou deja arretee.
    #[error("etat de session invalide: {0}")]
    InvalidState(&'static str),
    /// Erreur d'etat avec message dynamique (services, stack ipv8).
    #[error("etat: {0}")]
    State(String),
    /// Operation abandonnee volontairement : le magnet en resolution
    /// a ete supprime entre-temps (`pending` retire) — pas une
    /// defaillance, rien a remonter a l'utilisateur.
    #[error("operation annulee: {0}")]
    Cancelled(&'static str),
    /// Erreur cryptographique (cles IPv8).
    #[error("crypto: {0}")]
    Crypto(#[from] onionbit_crypto::CryptoError),
    /// Le tracker a repondu par un refus (`failure reason` HTTP ou
    /// action error UDP BEP-15, ex. « scrape disabled » sur les
    /// trackers prives) — il est **joignable** : pas une panne.
    #[error("refus du tracker: {0}")]
    ScrapeRefused(String),
    /// Refus impose par une politique reseau (anti-SSRF).
    #[error("politique reseau: {0}")]
    Policy(#[from] onionbit_network_policy::PolicyError),
}
