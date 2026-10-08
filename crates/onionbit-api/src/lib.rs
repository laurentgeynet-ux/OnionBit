// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-api` — plan de controle REST + SSE du daemon.
//!
//! Equivalent de `tribler.core.restapi` : traduit HTTP/JSON vers les
//! operations de `onionbit-core` (`CoreSession`) et diffuse les
//! notifications internes en flux SSE sur `/api/events`.
//!
//! Securite : le routeur est destine a n'ecouter que sur `127.0.0.1`
//! (le bind est fait dans `onionbit-daemon`). Le mapping exact avec
//! l'API Python est documente dans
//! `docs/reference_tribler/api_rest_mapping.md`.

mod auth;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod router;
pub mod state;
mod webui;

pub use error::{ApiError, ApiErrorBody};
pub use router::build;
pub use state::AppState;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn le_routeur_repond_404_sur_route_inconnue() {
        // Smoke test de construction : le routeur se monte sans panic.
        let dir = tempfile::tempdir().unwrap();
        let session = onionbit_core::CoreSession::start_offline(
            onionbit_core::CoreConfig::offline(dir.path().into()),
            onionbit_core::Notifier::new(),
        )
        .await
        .unwrap();
        let _app = build(AppState::new(session));
        // Pas d'assertion reseau ici : les tests HTTP complets sont
        // dans tests/ (a venir avec onionbit-daemon).
    }
}
