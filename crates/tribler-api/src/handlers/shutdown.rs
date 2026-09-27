//! Handler `/api/shutdown` — equivalent de
//! `tribler.core.restapi.shutdown_endpoint`.

use axum::extract::State;
use axum::Json;

use crate::state::AppState;

/// `PUT /api/shutdown` — demande l'arret de la session.
///
/// Comme le Python (`shutdown_request` → `events` notifie
/// `tribler_shutdown_started` puis l'arret est lance), la reponse part
/// avant l'arret effectif, qui s'execute en tache de fond.
pub async fn shutdown(State(state): State<AppState>) -> Json<serde_json::Value> {
    let session = state.session.clone();
    tokio::spawn(async move {
        session.stop().await;
    });
    Json(serde_json::json!({ "shutdown": true }))
}
