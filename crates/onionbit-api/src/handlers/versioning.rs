// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/versioning` — equivalent de
//! `tribler.core.versioning.restapi.versioning_endpoint`.
//!
//! Pas de gestionnaire de versions : le daemon rapporte sa version
//! courante et repond honnetement "aucune mise a jour disponible" aux
//! sondes (pas de trafic sortant implicite vers tribler.org/GitHub).

use axum::extract::{Path, State};
use axum::Json;

use crate::error::ApiError;
use crate::state::AppState;

/// `versioning/enabled` Python : la section desactivee n'enregistre
/// pas l'endpoint du tout — 404 comme une route absente.
fn gate(state: &AppState) -> Result<(), ApiError> {
    let enabled = state
        .daemon_config
        .lock()
        .map(|c| c.versioning.enabled)
        .unwrap_or(true);
    if enabled {
        Ok(())
    } else {
        Err(ApiError::not_found("versioning desactive"))
    }
}

/// `GET /api/versioning/versions` — sous-repertoires de version dans
/// `state_dir` (`get_versions` Python : `v8.x` presents).
pub async fn get_versions(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    let mut versions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&state.session.config().state_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() && name.starts_with('v') {
                versions.push(serde_json::json!(name));
            }
        }
    }
    Ok(Json(serde_json::json!({
        "versions": versions,
        "current": format!("v{}", env!("CARGO_PKG_VERSION")),
    })))
}

/// `GET /api/versioning/versions/current` — version courante
/// (`"git"` si non packagee, comme Python).
pub async fn get_current_version(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    Ok(Json(
        serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
    ))
}

/// `GET /api/versioning/versions/check` — sonde de mise a jour.
/// Pas de requete reseau : `has_version` est toujours `false`
/// (comportement documente, pas de trafic implicite).
/// `versioning/allow_pre` filtrera les pre-versions quand la
/// verification distante sera implementee (rien a filtrer pour
/// l'instant).
pub async fn check_version(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    Ok(Json(
        serde_json::json!({ "new_version": "", "has_version": false }),
    ))
}

/// `DELETE /api/versioning/versions/{version}` — suppression d'un
/// sous-repertoire de version dans `state_dir` (jamais le courant).
pub async fn remove_version(
    State(state): State<AppState>,
    Path(version): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    if version == format!("v{}", env!("CARGO_PKG_VERSION")) {
        return Err(ApiError::bad_request(
            "impossible de supprimer la version courante",
        ));
    }
    // Seuls les sous-repertoires `v*` de state_dir sont supprimables.
    let safe = version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    if !safe || !version.starts_with('v') {
        return Err(ApiError::bad_request("nom de version invalide"));
    }
    let dir = state.session.config().state_dir.join(&version);
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("version inconnue: {version}")));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("suppression impossible: {e}")))?;
    Ok(Json(serde_json::json!({ "removed": true })))
}

/// `POST /api/versioning/upgrade` — pas de migration disponible
/// (`perform_upgrade` Python repond `{"started": bool}`).
pub async fn perform_upgrade(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    Ok(Json(serde_json::json!({ "started": false })))
}

/// `GET /api/versioning/upgrade/available` — rien a migrer.
pub async fn can_upgrade(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    Ok(Json(serde_json::json!({ "can_upgrade": false })))
}

/// `GET /api/versioning/upgrade/working` — pas de migration en cours.
pub async fn is_upgrading(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    Ok(Json(serde_json::json!({ "working": false })))
}
