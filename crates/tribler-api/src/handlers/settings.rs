//! Handlers `/api/settings` — equivalent de
//! `tribler.core.restapi.settings_endpoint`.
//!
//! Python : `GET` retourne `config.configuration` (l'arbre complet
//! `TriblerConfig` + ports runtime) ; `POST` fait un merge recursif
//! (`_recursive_merge_settings`) puis `config.write()` (persiste
//! `configuration.json`) et applique les limites de session si la
//! section `libtorrent` est presente.

use axum::extract::State;
use axum::Json;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/settings` — arbre complet de `configuration.json`,
/// superpose des valeurs effectivement en cours (reglages appliques a
/// chaud, ports runtime).
pub async fn get_settings(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut cfg = state.daemon_config.lock().unwrap().clone();
    cfg.apply_runtime_view(&state.session.effective_config());
    Json(serde_json::json!({
        "settings": serde_json::to_value(&cfg).unwrap_or_default()
    }))
}

/// `POST /api/settings` — merge recursif du corps (les objets se
/// fusionnent, les feuilles remplacent — semantique `config.set("a/b")`
/// Python), puis persistance dans `configuration.json` et application
/// a chaud du sous-ensemble reconnu (flux RSS, watch folder, dossier
/// de telechargement).
///
/// Le corps peut etre l'arbre de reglages directement (comme Python)
/// ou enveloppe dans une cle `settings` (confort historique).
pub async fn update_settings(
    State(state): State<AppState>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = req.get("settings").unwrap_or(&req);
    if !patch.is_object() {
        return Err(ApiError::bad_request("settings must be a JSON object"));
    }

    let mut cfg = state.daemon_config.lock().unwrap();
    cfg.merge(patch)
        .map_err(|e| ApiError::bad_request(format!("invalid settings: {e}")))?;
    if let Some(path) = &state.config_path {
        cfg.write(path)
            .map_err(|e| ApiError::internal(format!("ecriture configuration.json: {e}")))?;
    }
    // Re-derive la config coeur depuis l'arbre persiste et applique le
    // sous-ensemble a chaud (RSS, watch folder, saveas).
    let core_cfg = cfg.to_core_config(&state.session.config().state_dir);
    drop(cfg);
    state.session.apply_service_settings(&core_cfg);
    // Resynchronise les autres clients SSE (editeur avance d'un
    // client → sections dediees des autres rafraichies).
    state
        .session
        .notifier()
        .notify(tribler_core::Notification::SettingsChanged);

    Ok(Json(serde_json::json!({ "modified": true })))
}
