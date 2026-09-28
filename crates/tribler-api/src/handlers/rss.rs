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

/// `GET /api/rss` — liste les items decouverts par les watchers
/// (`rss_items`). Extension Rust : l'endpoint Python ne propose que
/// `PUT` (cf. ADR-0006).
pub async fn list_items(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, crate::error::ApiError> {
    let items = state
        .session
        .db()
        .list_rss_items()
        .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "items": items
            .iter()
            .map(|i| serde_json::json!({
                "feed_url": i.feed_url,
                "link": i.link,
                "title": i.title,
                "infohash": i.infohash,
                "first_seen": i.first_seen,
            }))
            .collect::<Vec<_>>()
    })))
}
