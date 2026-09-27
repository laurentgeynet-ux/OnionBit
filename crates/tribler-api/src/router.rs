//! Construction du routeur axum.
//!
//! Les chemins suivent `tribler.core.restapi` (`/api/...`). Le serveur
//! ne doit etre expose que sur `127.0.0.1` (API de controle locale) —
//! voir `tribler-network-policy`.

use axum::routing::{delete, get, patch, put};
use axum::Router;

use crate::handlers::downloads;
use crate::handlers::events;
use crate::state::AppState;

/// Construit le routeur complet de l'API.
pub fn build(state: AppState) -> Router {
    Router::new()
        .route("/api/downloads", get(downloads::get_downloads))
        .route("/api/downloads", put(downloads::add_download))
        .route(
            "/api/downloads/{infohash}",
            delete(downloads::delete_download),
        )
        .route(
            "/api/downloads/{infohash}",
            patch(downloads::update_download),
        )
        .route("/api/events", get(events::get_events))
        .with_state(state)
}
