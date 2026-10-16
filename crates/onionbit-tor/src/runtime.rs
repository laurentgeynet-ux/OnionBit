// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime Tor : cycle de vie du client et fetch borne.
//!
//! A l'etape 112, seul le squelette existe — le transport
//! `arti-client` arrive a l'etape 113 derriere la feature `tor`.
//! Les invariants sont deja en place : bootstrap en tache de fond
//! jamais bloquant, etat observable pour `/api/tor/status`, et
//! `fetch_onion` ne produit jamais de trafic hors politique (il
//! refuse avant meme d'ouvrir une connexion).

use crate::error::{Result, TorError};
use crate::settings::TorSettings;

/// Etat observable du runtime (`GET /api/tor/status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorState {
    /// Tor desactive (`tor.enabled = false`) ou runtime non cree.
    Off,
    /// Bootstrap en cours (telechargement du consensus, gardes).
    Bootstrapping {
        /// Progression arti 0-100 quand connue.
        progress: u8,
    },
    /// Circuits utilisables.
    Ready,
    /// Client dormant (ecomode arti — conserve les gardes, degage
    /// les circuits inutilises).
    Dormant,
    /// Bootstrap ou transport en echec — daemon operationnel, la
    /// recherche externe rend une erreur explicite.
    Error(String),
}

/// Runtime Tor — proprietaire du `TorClient` arti (feature `tor`).
///
/// Construction paresseuse : `new` ne boote rien ; le bootstrap est
/// lance par [`TorRuntime::bootstrap`] en tache de fond, jamais au
/// demarrage du daemon ni dans un handler d'API (ADR-0028 §2 — un
/// blocage reseau ne doit jamais geler le daemon).
pub struct TorRuntime {
    settings: TorSettings,
    state: TorState,
}

impl TorRuntime {
    /// Cree le runtime sans bootstrap — `Off` tant que
    /// [`TorRuntime::bootstrap`] n'a pas ete appele (bootstrap
    /// explicite et paresseux, jamais au `new`).
    pub fn new(settings: TorSettings) -> Self {
        Self {
            settings,
            state: TorState::Off,
        }
    }

    /// Reglages actifs.
    pub fn settings(&self) -> &TorSettings {
        &self.settings
    }

    /// Etat courant (diagnostic `/api/tor/status`).
    pub fn state(&self) -> &TorState {
        &self.state
    }

    /// Lance le bootstrap en tache de fond. Etape 113 : reel via
    /// `arti-client` ; a ce stade, passe `state` a `Error` si la
    /// feature `tor` est absente.
    pub async fn bootstrap(&mut self) -> Result<()> {
        if !self.settings.enabled {
            return Err(TorError::Disabled);
        }
        if cfg!(feature = "tor") {
            // Etape 113 : TorClient::create_bootstrapped + spawn.
            self.state = TorState::Error("transport arti non implante (etape 113)".into());
            Err(TorError::BootstrapFailed(
                "transport arti non implante (etape 113)".into(),
            ))
        } else {
            self.state = TorState::Error("feature `tor` desactivee".into());
            Err(TorError::NotBuilt)
        }
    }

    /// Fetch borne d'une ressource `.onion` (`GET`/`HEAD`, reponse ≤
    /// `max_response_bytes`). La cible passe d'abord la
    /// [`onionbit_network_policy::TorTargetPolicy`] — aucune
    /// connexion n'est tentee hors allowlist, et il n'existe aucun
    /// repli clearnet.
    pub async fn fetch_onion(&self, url: &str) -> Result<Vec<u8>> {
        if !self.settings.enabled {
            return Err(TorError::Disabled);
        }
        if cfg!(feature = "tor") {
            let _ = url;
            Err(TorError::Request(
                "transport arti non implante (etape 113)".into(),
            ))
        } else {
            Err(TorError::NotBuilt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_desactive_refuse_sans_fuite() {
        let rt = TorRuntime::new(TorSettings::default());
        assert_eq!(rt.state(), &TorState::Off);
        assert!(matches!(
            rt.fetch_onion("http://x.onion").await,
            Err(TorError::Disabled)
        ));
    }

    #[tokio::test]
    async fn bootstrap_sans_transport_erreur_explicite() {
        let mut rt = TorRuntime::new(TorSettings {
            enabled: true,
            ..TorSettings::default()
        });
        let err = rt.bootstrap().await.unwrap_err();
        assert!(matches!(
            err,
            TorError::NotBuilt | TorError::BootstrapFailed(_)
        ));
        assert!(matches!(rt.state(), TorState::Error(_)));
    }
}
