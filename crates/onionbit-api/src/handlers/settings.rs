// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
    let mut cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
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

    let mut cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Merge sur une copie : une validation refusee ne doit pas
    // laisser l'arbre persiste a moitie mute.
    let mut candidate = cfg.clone();
    candidate
        .merge(patch)
        .map_err(|e| ApiError::bad_request(format!("invalid settings: {e}")))?;
    // ADR-0016 : `identity.at_rest` exige un redemarrage surveille —
    // incompatible avec `stealth.role != "client"` dans les deux
    // sens (activer at-rest sur un pont, ou passer un client at-rest
    // en pont). Refus ferme `400`.
    if candidate.identity.at_rest && candidate.stealth.enabled && candidate.stealth.role != "client"
    {
        return Err(ApiError::bad_request(
            "identity.at_rest est incompatible avec stealth.role != \"client\" (un pont doit redemarrer sans surveillance)",
        ));
    }
    *cfg = candidate;
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
        .notify(onionbit_core::Notification::SettingsChanged);

    Ok(Json(serde_json::json!({ "modified": true })))
}
