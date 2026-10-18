// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/versioning` — equivalent de
//! `tribler.core.versioning.restapi.versioning_endpoint`.
//!
//! Pas de gestionnaire de versions : le daemon rapporte sa version
//! courante. **`versions/check` est volontairement absent**
//! (divergence assumée, décision vie privée 2026-10) : une sonde de
//! mise à jour fuit en clair vers le serveur de releases
//! « une instance OnionBit tourne à cette IP » — de la télémétrie
//! incompatible avec le projet. Aucun trafic sortant de versioning
//! n'existe donc ; la route répond 404 comme une section absente.

use axum::extract::{Path, State};
use axum::http::StatusCode;
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

// ------------------------------------------------------------------
// Import `OBUP` (ADR-0032, etape 143)
//
// Le payload arrive par le **systeme de fichiers** : l'operateur
// depose le bundle (fichiers + `update.obup`) dans
// `state/update/incoming/` puis appelle l'import. Pas d'upload
// multipart (surface DoS — les fichiers contourneraient la borne
// `MAX_OBUP_SIZE`) ni de chemin en parametre (un `path` arbitraire
// serait une lecture de fichier pilotee par l'API).
//
// Ordre de verification — du moins cher au plus cher :
// signature/`key_id` → anti-downgrade → sha256 par fichier →
// staging atomique.
// ------------------------------------------------------------------

/// Traduit une erreur `OBUP` en `ApiError` codee — pas de
/// « corruption » generique sur un probleme de signature.
fn obup_err(e: onionbit_format::obupd::ObupdError) -> ApiError {
    use onionbit_format::obupd::ObupdError::*;
    let (status, code) = match &e {
        Malformed(_) | BadPath(_) | Format(_) => (StatusCode::BAD_REQUEST, "obup_malformed"),
        UnknownKeyId(_) => (StatusCode::UNPROCESSABLE_ENTITY, "obup_unknown_key"),
        SigInvalid => (StatusCode::UNPROCESSABLE_ENTITY, "obup_sig_invalid"),
    };
    ApiError::with_code(status, code, e.to_string())
}

/// `POST /api/versioning/import` — verifie et stage le bundle depose
/// dans `state/update/incoming/` (fichiers + `update.obup`).
///
/// Reponses codees : 400 `obup_malformed`/`obup_bad_path`, 413
/// `obup_too_large` (blob ou total stagé), 409 `obup_version_too_old`
/// /`obup_min_supported`, 422 `obup_unknown_key`/`obup_sig_invalid`
/// /`obup_missing_file`/`obup_hash_mismatch` (le fichier fautif est
/// nomme dans le message).
pub async fn import_update(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use onionbit_format::obupd as obup;
    use sha2::Digest;

    gate(&state)?;
    let state_dir = state.session.config().state_dir.clone();
    let update_dir = state_dir.join("update");
    let incoming = update_dir.join("incoming");
    let blob_path = incoming.join("update.obup");
    if !blob_path.is_file() {
        return Err(ApiError::not_found(
            "aucun artefact : deposer update.obup + fichiers dans state/update/incoming/",
        ));
    }

    // 1) Borne avant lecture — le blob est petit par construction.
    let meta = std::fs::metadata(&blob_path)
        .map_err(|e| ApiError::internal(format!("update.obup illisible : {e}")))?;
    if meta.len() as usize > obup::MAX_OBUP_SIZE {
        return Err(ApiError::with_code(
            StatusCode::PAYLOAD_TOO_LARGE,
            "obup_too_large",
            "blob OBUP au-dela de la borne",
        ));
    }
    let blob = std::fs::read(&blob_path)
        .map_err(|e| ApiError::internal(format!("update.obup illisible : {e}")))?;

    // 2) Signature + key_id (cheap — rejette vite).
    let keys = onionbit_crypto::release::accepted_keys(&state_dir);
    let manifest = obup::verify(&blob, &keys).map_err(obup_err)?;

    // 3) Anti-downgrade : `v > current`, sauf `rollback` signe ;
    //    `min_supported` borne l'instance trop vieille.
    let current = env!("CARGO_PKG_VERSION");
    match obup::semver_cmp(&manifest.version, current) {
        None => {
            return Err(ApiError::with_code(
                StatusCode::BAD_REQUEST,
                "obup_malformed",
                "version du manifest non-semver",
            ));
        }
        Some(std::cmp::Ordering::Greater) => {}
        Some(_) if manifest.rollback => {}
        Some(_) => {
            return Err(ApiError::with_code(
                StatusCode::CONFLICT,
                "obup_version_too_old",
                format!(
                    "{} n'est pas superieure a la version courante",
                    manifest.version
                ),
            ));
        }
    }
    if let Some(ms) = &manifest.min_supported {
        match obup::semver_cmp(current, ms) {
            Some(std::cmp::Ordering::Less) => {
                return Err(ApiError::with_code(
                    StatusCode::CONFLICT,
                    "obup_min_supported",
                    format!("version courante {current} < min_supported {ms}"),
                ));
            }
            None => {
                return Err(ApiError::with_code(
                    StatusCode::BAD_REQUEST,
                    "obup_malformed",
                    "min_supported non-semver",
                ));
            }
            _ => {}
        }
    }

    // 4) Borne de volume avant de hasher — la signature protege
    //    l'integrite, pas le disque.
    let max_bytes = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .versioning
        .update_max_bytes;
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    if total > max_bytes {
        return Err(ApiError::with_code(
            StatusCode::PAYLOAD_TOO_LARGE,
            "obup_too_large",
            format!("payload stagé {total}o > borne {max_bytes}o"),
        ));
    }

    // 5) sha256/size par fichier (couteux — en dernier). Le chemin
    //    est deja valide par le manifest (`..`/absolu interdits).
    for f in &manifest.files {
        let src = incoming.join(&f.path);
        let data = std::fs::read(&src).map_err(|_| {
            ApiError::with_code(
                StatusCode::UNPROCESSABLE_ENTITY,
                "obup_missing_file",
                format!("fichier du manifest absent : {}", f.path),
            )
        })?;
        if data.len() as u64 != f.size {
            return Err(ApiError::with_code(
                StatusCode::UNPROCESSABLE_ENTITY,
                "obup_hash_mismatch",
                format!("taille divergente : {}", f.path),
            ));
        }
        let sha: [u8; 32] = sha2::Sha256::digest(&data).into();
        if sha != f.sha256 {
            return Err(ApiError::with_code(
                StatusCode::UNPROCESSABLE_ENTITY,
                "obup_hash_mismatch",
                format!("sha256 divergent : {}", f.path),
            ));
        }
    }

    // 6) Staging atomique : tmp + rename (meme FS → atomique). Le
    //    re-import est idempotent : l'ancien staging est remplace.
    let payload_tmp = update_dir.join("payload.tmp");
    let payload_dir = update_dir.join("payload");
    if payload_tmp.exists() {
        std::fs::remove_dir_all(&payload_tmp)
            .map_err(|e| ApiError::internal(format!("purge payload.tmp : {e}")))?;
    }
    std::fs::create_dir_all(&payload_tmp)
        .map_err(|e| ApiError::internal(format!("staging : {e}")))?;
    for f in &manifest.files {
        let dst = payload_tmp.join(&f.path);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::internal(format!("staging : {e}")))?;
        }
        std::fs::copy(incoming.join(&f.path), &dst)
            .map_err(|e| ApiError::internal(format!("staging {} : {e}", f.path)))?;
    }
    if payload_dir.exists() {
        std::fs::remove_dir_all(&payload_dir)
            .map_err(|e| ApiError::internal(format!("purge ancien payload : {e}")))?;
    }
    std::fs::rename(&payload_tmp, &payload_dir)
        .map_err(|e| ApiError::internal(format!("swap staging : {e}")))?;
    let staged_tmp = update_dir.join("staged.obup.tmp");
    std::fs::write(&staged_tmp, &blob)
        .map_err(|e| ApiError::internal(format!("staged.obup : {e}")))?;
    std::fs::rename(&staged_tmp, update_dir.join("staged.obup"))
        .map_err(|e| ApiError::internal(format!("staged.obup : {e}")))?;

    // 7) Rotation : `next_key` signee persiste la transition — la
    //    clef successeur entre dans `accepted_keys` des maintenant.
    if let Some(nk) = &manifest.next_key {
        let ktmp = state_dir.join("release_key.next.tmp");
        std::fs::write(&ktmp, nk)
            .map_err(|e| ApiError::internal(format!("release_key.next : {e}")))?;
        std::fs::rename(&ktmp, state_dir.join("release_key.next"))
            .map_err(|e| ApiError::internal(format!("release_key.next : {e}")))?;
        tracing::info!(
            key_id = format!("{:#x}", onionbit_crypto::release::release_key_id(nk)),
            "clef de release successeur persistee"
        );
    }

    tracing::info!(
        version = %manifest.version,
        fichiers = manifest.files.len(),
        key_id = format!("{:#x}", manifest.key_id),
        "mise a jour OBUP stagee — verifiee a l'import, re-verifiee par le launcher"
    );
    tracing::debug!(dir = %update_dir.display(), "staging update");
    Ok(Json(serde_json::json!({
        "staged": true,
        "version": manifest.version,
        "target": manifest.target,
        "files": manifest.files.len(),
        "key_id": format!("{:#x}", manifest.key_id),
        "next_key_rotation": manifest.next_key.is_some(),
    })))
}

/// `GET /api/versioning/update` — statut du staging : blob parse ET
/// verifie contre les clefs acceptees (l'affichage ne fait pas foi
/// au swap — le launcher re-verifie, mais un staging corrompu est
/// signale honnetement).
pub async fn get_update_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gate(&state)?;
    let state_dir = state.session.config().state_dir.clone();
    let staged = state_dir.join("update").join("staged.obup");
    let blob = match std::fs::read(&staged) {
        Ok(b) => b,
        Err(_) => return Ok(Json(serde_json::json!({ "staged": false }))),
    };
    let keys = onionbit_crypto::release::accepted_keys(&state_dir);
    match onionbit_format::obupd::verify(&blob, &keys) {
        Ok(m) => Ok(Json(serde_json::json!({
            "staged": true,
            "verified": true,
            "version": m.version,
            "target": m.target,
            "files": m.files.len(),
            "key_id": format!("{:#x}", m.key_id),
        }))),
        Err(_) => Ok(Json(serde_json::json!({
            "staged": false,
            "invalid": true,
        }))),
    }
}
