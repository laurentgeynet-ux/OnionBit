// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-tor` — connectivite Tor embarquee (ADR-0028, extension
//! hors fidelite Tribler).
//!
//! Proprietaire unique de tout trafic Tor du daemon :
//!
//! - [`settings::TorSettings`] : reglages runtime issus de la section
//!   `tor` de `configuration.json` (mode embarque arti / externe
//!   SOCKS5, allowlist de cibles, backends de recherche, passerelle) ;
//! - [`runtime::TorRuntime`] : cycle de vie du client Tor (bootstrap
//!   en tache de fond — jamais bloquant, dormant entre usages) et
//!   fetch borne [`TorRuntime::fetch_onion`].
//!
//! Frontieres dures : toute cible passe par
//! `onionbit_network_policy::TorTargetPolicy` (hotes `.onion` v3
//! allowlistes, ports 80/443) et il n'existe **aucun** repli
//! clearnet — Tor indisponible rend une erreur, pas une fuite.
//!
//! Le transport reel (`arti-client` embarque et `tokio-socks` en
//! mode externe) est derriere le feature flag `tor` : sans lui, le
//! crate compile leger et `fetch_onion` rend [`TorError::NotBuilt`]
//! (ADR-0028 §1 — le poids de build d'arti est inacceptable en
//! dependance obligatoire). Le decodeur HTTP/1.1 borne ([`http`],
//! `httparse`) compile dans tous les cas — ses tests n'ont besoin
//! d'aucun transport.

pub mod error;
// Le decodeur HTTP borne compile dans tous les cas (ses tests
// tournent sans transport) — sans feature `tor` ni test, ses
// fonctions n'ont pas encore d'appelant : c'est attendu.
#[cfg_attr(not(any(feature = "tor", test)), allow(dead_code))]
pub(crate) mod http;
pub mod runtime;
pub mod search;
// Service `.onion` heberge (ADR-0029 etape 122) — feature `hs`.
#[cfg(feature = "hs")]
pub(crate) mod service;
pub mod settings;

pub use error::{ExternalSearchError, Result, TorError};
pub use http::OnionResponse;
pub use runtime::{OnionServiceInfo, TorRuntime, TorState};
pub use search::{build_backends, ExternalResult, OnionFetch, OnionSearchBackend};
pub use settings::{
    OnionBackendSettings, OnionServiceSettings, TorGatewaySettings, TorMode, TorSettings,
};
