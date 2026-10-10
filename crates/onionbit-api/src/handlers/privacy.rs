// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/privacy/profile` (ADR-0022) — extension OnionBit
//! sans equivalent `tribler.core.restapi` : bascule de la posture
//! d'anonymat globale entre les presets `legacy` / `full` /
//! `custom` definis dans `onionbit_core::privacy`.
//!
//! - `GET` : lecture libre (y compris en session invitee) du profil
//!   stocke, du profil **effectif** derive (`custom` des qu'une cle
//!   couverte diverge du preset) et du signal `restart_pending`.
//! - `PUT` : preset materialise — tout-ou-rien sur copie puis
//!   persistance atomique, meme discipline que `POST /api/settings`.
//!   `409 guest_session` en session invitee (zero artefact ADR-0016) ;
//!   `409 missing_prerequisites` (+ `missing:[...]`) si `full` sans
//!   pont ; `400` profil inconnu.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use onionbit_core::privacy::{self, PrivacyError, PrivacyProfile};

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/privacy/profile` — posture d'anonymat : intention
/// persistee (`stored`), profil effectif derive, cles divergentes,
/// redemarrage en attente et nombre de ponts stealth configures.
pub async fn get_profile(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (effective, diverged) = PrivacyProfile::effective(&cfg);
    Json(serde_json::json!({
        "stored": cfg.privacy.profile,
        "effective": effective,
        "diverged_keys": diverged,
        "restart_pending": privacy::restart_pending(&cfg, &state.startup_config),
        "guest": state.session.is_guest(),
        "stealth": {
            "bridges_configured": cfg.stealth.bridges.len(),
        },
    }))
}

/// `PUT /api/privacy/profile` `{profile}` — bascule de posture :
/// `apply()` materialise le preset sur une copie (prerequis + gardes
/// de combinaison verifies avant toute mutation), puis persistance
/// et application a chaud du sous-ensemble reconnu.
pub async fn put_profile(
    State(state): State<AppState>,
    Json(req): Json<serde_json::Value>,
) -> Response {
    // Une session invitee ne persiste jamais dans le
    // `configuration.json` du proprietaire (zero artefact ADR-0016)
    // — et un profil non persiste serait mensonger : les cles
    // structurantes sont toutes a redemarrage.
    if state.session.is_guest() {
        return ApiError::conflict("guest_session").into_response();
    }
    let target: PrivacyProfile = match req
        .get("profile")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
    {
        Ok(Some(p)) => p,
        _ => {
            return ApiError::bad_request(
                "profile inconnu — attendu \"legacy\" | \"full\" | \"custom\"",
            )
            .into_response()
        }
    };

    let mut cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let stored_before = cfg.privacy.profile;
    let (next, outcome) = match PrivacyProfile::apply(&cfg, target) {
        Ok(ok) => ok,
        Err(PrivacyError::MissingPrerequisites(missing)) => {
            // Corps `{error}` Tribler + `missing` lisible machine :
            // l'UI ouvre le dialogue de saisie de pont sur
            // `stealth.bridges`.
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": {"handled": true, "message": "missing_prerequisites"},
                    "missing": missing,
                })),
            )
                .into_response();
        }
        Err(PrivacyError::InvalidCombination(msg)) => {
            return ApiError::conflict(msg).into_response();
        }
        Err(PrivacyError::Merge(e)) => {
            return ApiError::bad_request(format!("preset invalide: {e}")).into_response();
        }
    };

    let (effective, diverged) = PrivacyProfile::effective(&next);
    let modified = !outcome.applied_keys.is_empty() || stored_before != target;
    *cfg = next;
    if let Some(path) = &state.config_path {
        if let Err(e) = cfg.write(path) {
            return ApiError::internal(format!("ecriture configuration.json: {e}")).into_response();
        }
    }
    // Sous-ensemble a chaud (hops, guards, ledger, zone par defaut)
    // — les cles froides attendent le redemarrage signale.
    let core_cfg = cfg.to_core_config(&state.session.config().state_dir);
    drop(cfg);
    state.session.apply_service_settings(&core_cfg);
    state
        .session
        .notifier()
        .notify(onionbit_core::Notification::SettingsChanged);

    Json(serde_json::json!({
        "modified": modified,
        "profile": target,
        "effective": effective,
        "diverged_keys": diverged,
        "restart_required": outcome.restart_required,
        "applied_keys": outcome.applied_keys,
    }))
    .into_response()
}
