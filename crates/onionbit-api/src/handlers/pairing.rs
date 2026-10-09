// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `POST /api/pairing/*` — appairage mobile par QR (ADR-0021 §8,
//! etape 76). L'app Android/iOS pilote un daemon desktop a distance
//! (modele « telecommande » du projet) ; plutot que saisir l'adresse
//! et la cle API sur un clavier tactile, le desktop affiche un QR
//! encodant `host:port` + un jeton a courte duree de vie et usage
//! unique, que `redeem` echange contre la cle API.
//!
//! `token` est derriere `api_key_auth` (le desktop qui affiche le QR
//! est deja authentifie) ; `redeem` est la seule route `/api`
//! exemptee — le mobile ne possede pas encore la cle, c'est
//! precisement ce qu'il vient chercher (voir `auth.rs`). La borne de
//! securite est le jeton lui-meme : 128 bits aleatoires, TTL court
//! (`api.pairing.token_ttl_secs`), usage unique, et `redeem` est
//! rate-limite par IP + globalement (`api.pairing.redeem_*`).

use std::net::IpAddr;
use std::time::Duration;

use axum::extract::State;
use axum::Json;

use super::identity::MaybeClientIp;
use crate::error::ApiError;
use crate::state::AppState;

/// Corps `{token}` de `POST /api/pairing/redeem` — le jeton hex du
/// QR scanne.
#[derive(serde::Deserialize)]
pub struct RedeemBody {
    /// Jeton d'appairage.
    pub token: String,
}

/// `POST /api/pairing/token` — emet le jeton que le desktop affiche
/// dans le QR. Sans cle API configuree l'appairage n'a pas de sens
/// (l'API est ouverte : le mobile se connecte directement sans
/// jeton) — `409 pairing_disabled` pour signaler franchement ce cas.
pub async fn post_token(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.api_key.as_deref().filter(|k| !k.is_empty()).is_none() {
        return Err(ApiError::conflict("pairing_disabled"));
    }
    let ttl_secs = {
        let cfg = state
            .daemon_config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cfg.api.pairing.token_ttl_secs
    };
    // 128 bits aleatoires — la borne anti brute-force est le jeton
    // (TTL court + usage unique + rate-limit de `redeem`).
    let token = uuid::Uuid::new_v4().simple().to_string();
    state
        .pairing
        .issue(token.clone(), Duration::from_secs(ttl_secs));
    Ok(Json(serde_json::json!({
        "token": token,
        "expires_in_secs": ttl_secs,
    })))
}

/// `POST /api/pairing/redeem` — echange le jeton du QR contre la cle
/// API du daemon. Erreur uniforme 401 : invalide, perime ou deja
/// consomme ne se distinguent pas pour l'appelant (rien n'est revele
/// sur l'existence d'un grant).
pub async fn post_redeem(
    State(state): State<AppState>,
    MaybeClientIp(client_ip): MaybeClientIp,
    Json(body): Json<RedeemBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(api_key) = state.api_key.as_deref().filter(|k| !k.is_empty()) else {
        return Err(ApiError::conflict("pairing_disabled"));
    };
    let api_key = api_key.to_owned();
    let (per_ip_max, global_max, window_secs) = {
        let cfg = state
            .daemon_config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            cfg.api.pairing.redeem_per_ip_per_min,
            cfg.api.pairing.redeem_global_per_min,
            cfg.api.pairing.redeem_window_secs,
        )
    };
    let ip = client_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    if !state
        .pairing_limiter
        .admit(ip, per_ip_max, global_max, Duration::from_secs(window_secs))
    {
        return Err(ApiError::too_many_requests(
            "trop de tentatives d'appairage",
        ));
    }
    if !state.pairing.redeem(&body.token) {
        return Err(ApiError::unauthorized());
    }
    Ok(Json(serde_json::json!({ "api_key": api_key })))
}
