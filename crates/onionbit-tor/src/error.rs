// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs typees de `onionbit-tor`.

/// Erreur de la connectivite Tor.
#[derive(Debug, thiserror::Error)]
pub enum TorError {
    /// Le transport arti n'est pas compile (feature `tor` absente —
    /// ADR-0028 §1).
    #[error("transport Tor non compile (feature `tor` desactivee)")]
    NotBuilt,

    /// Tor est desactive (`tor.enabled = false`) — pas de repli
    /// possible : jamais de fallback clearnet.
    #[error("connectivite Tor desactivee")]
    Disabled,

    /// Le bootstrap Tor a echoue (reseau filtre, FAI bloquant, mode
    /// externe injoignable).
    #[error("bootstrap Tor echoue : {0}")]
    BootstrapFailed(String),

    /// Cible refusee par la politique d'allowlist onion.
    #[error("cible refusee : {0}")]
    DeniedTarget(#[from] onionbit_network_policy::PolicyError),

    /// Echec d'une requete vers un service cache.
    #[error("requete onion echouee : {0}")]
    Request(String),
}

/// Alias de `Result` pour ce crate.
pub type Result<T> = std::result::Result<T, TorError>;
