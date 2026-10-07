// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `GET/POST /api/identity/*` — identite IPv8 portable (export /
//! import de la cle secrete).
//!
//! Export : `LibNaCLSK:` brut en hex, ou blob `OBID` (argon2id +
//! ChaCha20-Poly1305) quand un mot de passe est fourni — c'est le
//! blob qui voyage entre devices, donc c'est lui qui merite la
//! protection. Import : valide le format, ecrase
//! `ipv8_keypair.bin`, `restart_required` (la cle est liee aux
//! communautes en cours d'execution).
//!
//! Endpoints sensibles : derriere `api_key_auth` comme le reste de
//! `/api` ; jamais loggues, jamais dans les events SSE.

use axum::extract::State;
use axum::Json;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_crypto::keyblob::{keyblob_is_sealed, keyblob_open, keyblob_seal};

use crate::error::ApiError;
use crate::state::AppState;

/// Corps `{password?}` de `POST /api/identity/export` — vide/absent
/// = export de la cle brute (l'utilisateur gere la protection).
#[derive(serde::Deserialize)]
pub struct IdentityExportBody {
    /// Mot de passe de protection du blob (optionnel).
    #[serde(default)]
    pub password: Option<String>,
}

/// Corps `{key, password?}` de `POST /api/identity/restore`.
#[derive(serde::Deserialize)]
pub struct IdentityRestoreBody {
    /// Cle hex — `LibNaCLSK:` brut ou blob `OBID…`.
    pub key: String,
    /// Mot de passe si le blob est chiffre `OBID`.
    #[serde(default)]
    pub password: Option<String>,
}

/// `GET /api/identity` — cle publique de l'identite courante (hex).
/// 404 si IPv8 est desactive (pas d'identite chargee).
pub async fn get_identity(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    Ok(Json(serde_json::json!({
        "public_key": stack.public_key_hex(),
        // La cle privee n'est jamais exposee en lecture simple —
        // seul l'export explicite la fournit.
    })))
}

/// `POST /api/identity/export` — cle secrete `LibNaCLSK:` (hex brut)
/// ou blob `OBID` si `password` fourni. `{key, encrypted}`.
pub async fn export_identity(
    State(state): State<AppState>,
    Json(body): Json<IdentityExportBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    let raw = stack.secret_key_bin();
    let password = body.password.unwrap_or_default();
    if password.is_empty() {
        return Ok(Json(serde_json::json!({
            "key": hex::encode(&raw),
            "encrypted": false,
        })));
    }
    let blob = keyblob_seal(password.as_bytes(), &raw)
        .map_err(|_| ApiError::bad_request("export impossible"))?;
    Ok(Json(serde_json::json!({
        "key": hex::encode(&blob),
        "encrypted": true,
    })))
}

/// `POST /api/identity/restore` — remplace `ipv8_keypair.bin` par la
/// cle fournie (brute ou `OBID` + `password`). Repond
/// `{restart_required: true}` : la nouvelle identite est activee au
/// prochain demarrage du daemon.
pub async fn restore_identity(
    State(state): State<AppState>,
    Json(body): Json<IdentityRestoreBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let blob =
        hex::decode(body.key.trim()).map_err(|_| ApiError::bad_request("cle attendue en hex"))?;
    let raw = if keyblob_is_sealed(&blob) {
        let password = body.password.unwrap_or_default();
        if password.is_empty() {
            return Err(ApiError::bad_request(
                "blob OBID chiffre — mot de passe requis",
            ));
        }
        keyblob_open(password.as_bytes(), &blob)
            .map_err(|_| ApiError::bad_request("dechiffrement impossible (mot de passe ?)"))?
    } else {
        blob
    };
    // Validation format avant toute ecriture.
    LibNaClSecretKey::from_bin(&raw)
        .map_err(|_| ApiError::bad_request("cle LibNaCLSK mal formee"))?;
    let state_dir = state.session.config().state_dir.clone();
    onionbit_core::ipv8_stack::restore_identity_key(&state_dir, &raw)
        .map_err(|e| ApiError::bad_request(format!("ecriture impossible: {e}")))?;
    Ok(Json(serde_json::json!({ "restart_required": true })))
}
