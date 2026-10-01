// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs typees des politiques reseau.

/// Erreur de politique reseau.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// Destination refusee par la politique d'adresses (anti-SSRF /
    /// SOCKS5).
    #[error("destination refusee ({0})")]
    DeniedDestination(&'static str),

    /// Le kill switch est engage : tout trafic conditionne par
    /// l'anonymat est bloque.
    #[error("kill switch engage : {0}")]
    KillSwitchEngaged(String),

    /// URL de proxy invalide (schema, port ou hote hors politique).
    #[error("proxy invalide : {0}")]
    InvalidProxy(&'static str),
}

/// Resultat type des politiques.
pub type Result<T> = std::result::Result<T, PolicyError>;
