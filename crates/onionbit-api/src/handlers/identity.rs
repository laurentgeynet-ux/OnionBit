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

use axum::extract::{Query, State};
use axum::Json;
use onionbit_core::identity::IdentityKind;
use onionbit_crypto::identity::IdentitySeed;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_crypto::keyblob::{keyblob_is_sealed, keyblob_open, keyblob_seal};
use onionbit_format::bip39::{self, Language};

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

/// Corps `{key, password?}` ou `{phrase}` de
/// `POST /api/identity/restore` — exactement un des deux.
#[derive(serde::Deserialize)]
pub struct IdentityRestoreBody {
    /// Cle hex — `LibNaCLSK:` brut ou blob `OBID…`.
    #[serde(default)]
    pub key: Option<String>,
    /// Phrase de recuperation BIP39 (24 mots, EN ou FR) — installe
    /// `identity_seed.bin` et regenere les cles derivees (ADR-0016).
    #[serde(default)]
    pub phrase: Option<String>,
    /// Mot de passe si le blob est chiffre `OBID`.
    #[serde(default)]
    pub password: Option<String>,
    /// Confirmation explicite de la conversion seedee → legacy :
    /// requis quand `identity_seed.bin` existe et que le restore se
    /// fait par `key` (la graine gagnerait sinon au prochain boot).
    #[serde(default)]
    pub force_legacy: bool,
}
#[derive(serde::Deserialize)]
pub struct RecoveryPhraseQuery {
    /// `en` (defaut) ou `fr` — wordlist d'encodage.
    #[serde(default)]
    pub lang: Option<String>,
}

/// `GET /api/identity` — cle publique de l'identite courante (hex)
/// et `seeded` : `true` si l'identite est racinee sur
/// `identity_seed.bin` (phrase de recuperation disponible,
/// ADR-0016). 404 si IPv8 est desactive (pas d'identite chargee).
pub async fn get_identity(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    Ok(Json(serde_json::json!({
        "public_key": stack.public_key_hex(),
        "seeded": stack.identity_kind() == IdentityKind::Seeded,
        // La cle privee n'est jamais exposee en lecture simple —
        // seul l'export explicite la fournit.
    })))
}

/// `GET /api/identity/recovery_phrase?lang=en|fr` — phrase BIP39 de
/// 24 mots de l'identite seedee. **Secret complet** : derriere
/// `api_key_auth` comme tout `/api`, jamais loggé, jamais dans les
/// events SSE. 404 sur une identite legacy (pas de graine = pas de
/// phrase — la seule sauvegarde reste l'export `OBID`).
pub async fn recovery_phrase(
    State(state): State<AppState>,
    Query(q): Query<RecoveryPhraseQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    if stack.identity_kind() != IdentityKind::Seeded {
        return Err(ApiError::not_found(
            "identite legacy — pas de phrase de recuperation (export OBID uniquement)",
        ));
    }
    let lang = match q.lang.as_deref().unwrap_or("en") {
        "en" => Language::English,
        "fr" => Language::French,
        other => return Err(ApiError::bad_request(format!("langue inconnue : {other}"))),
    };
    let state_dir = state.session.config().state_dir.clone();
    let raw = std::fs::read(onionbit_core::identity::seed_path(&state_dir))
        .map_err(|_| ApiError::internal("identity_seed.bin illisible"))?;
    let seed = IdentitySeed::from_bytes(&raw)
        .map_err(|_| ApiError::internal("identity_seed.bin corrompu"))?;
    // Revelation d'un secret complet — evenement notable (tracé warn,
    // la phrase elle-meme n'est JAMAIS logguee).
    tracing::warn!("phrase de recuperation revelee via l'API");
    Ok(Json(serde_json::json!({
        "phrase": bip39::encode(seed.as_bytes(), lang),
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

/// `POST /api/identity/restore` — deux formes exclusives :
///
/// - `{phrase}` : phrase BIP39 → installe `identity_seed.bin` et
///   regenere `ipv8_keypair.bin` + `stealth_bridge.key` (ADR-0016) ;
/// - `{key, password?}` : cle `LibNaCLSK:` hex brute ou blob `OBID`
///   (chemin legacy — refuse sur install seedee, la graine
///   gagnerait au prochain boot).
///
/// Repond `{restart_required: true}` : la nouvelle identite est
/// activee au prochain demarrage du daemon.
pub async fn restore_identity(
    State(state): State<AppState>,
    Json(body): Json<IdentityRestoreBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_dir = state.session.config().state_dir.clone();
    if let Some(phrase) = body.phrase {
        if body.key.is_some() {
            return Err(ApiError::bad_request("phrase OU key, pas les deux"));
        }
        // Validation checksum AVANT toute ecriture.
        let entropy = bip39::decode(&phrase)
            .map_err(|e| ApiError::bad_request(format!("phrase invalide : {e}")))?;
        let seed = IdentitySeed::from_bytes(&entropy)
            .map_err(|_| ApiError::bad_request("entropie de phrase invalide"))?;
        onionbit_core::identity::restore_seed(&state_dir, &seed)
            .map_err(|e| ApiError::bad_request(format!("ecriture impossible: {e}")))?;
        return Ok(Json(serde_json::json!({ "restart_required": true })));
    }
    let Some(key) = body.key else {
        return Err(ApiError::bad_request("phrase ou key requis"));
    };
    let blob = hex::decode(key.trim()).map_err(|_| ApiError::bad_request("cle attendue en hex"))?;
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
    onionbit_core::ipv8_stack::restore_identity_key(&state_dir, &raw, body.force_legacy)
        .map_err(|e| ApiError::bad_request(format!("ecriture impossible: {e}")))?;
    Ok(Json(serde_json::json!({ "restart_required": true })))
}
