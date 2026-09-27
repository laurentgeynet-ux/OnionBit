//! Handler `/api/rss` — equivalent de
//! `tribler.core.rss.restapi.endpoint` (`update_feeds`).

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::state::AppState;

/// Corps de `PUT /api/rss` (`{"urls": [...]}` Python).
#[derive(Debug, Deserialize)]
pub struct UpdateFeedsRequest {
    /// URLs des flux surveilles.
    pub urls: Vec<String>,
}

/// `PUT /api/rss` — remplace la liste des flux surveilles. Cree le
/// `RssManager` a la volee si le service n'etait pas configure au
/// demarrage.
pub async fn update_feeds(
    State(state): State<AppState>,
    Json(req): Json<UpdateFeedsRequest>,
) -> Json<serde_json::Value> {
    let mut cfg = state.session.effective_config();
    cfg.rss_urls = req.urls;
    state.session.apply_service_settings(&cfg);
    Json(serde_json::json!({ "modified": true }))
}
