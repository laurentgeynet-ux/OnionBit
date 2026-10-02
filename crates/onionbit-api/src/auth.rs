// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Middleware d'authentification — equivalent de
//! `rest_manager.ApiKeyMiddleware` Python.
//!
//! La cle est cherchee dans (par ordre) :
//! - l'en-tete `X-Api-Key`,
//! - le parametre de query `key`,
//! - le cookie `api_key`.
//!
//! Echec → 401 `{"error": {"handled": true, "message": "Unauthorized
//! access"}}`. Cle configuree vide → tout passe (semantique Python :
//! `not expected_api_key`). Les exemptions `/docs`, `/static`, `/ui`
//! du Python ont leur equivalent cote routeur : les statiques de l'UI
//! web sont servies en fallback **hors `/api`** (`webui.rs`) et ne
//! passent donc jamais par ce middleware — `/api/*` reste protege.

use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::state::AppState;

/// Middleware axum : rejette les requetes sans cle API valide.
pub async fn api_key_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.api_key.as_deref().filter(|k| !k.is_empty()) else {
        return next.run(req).await;
    };

    if provided_key(&req).as_deref() == Some(expected) {
        next.run(req).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

/// Cle fournie par la requete (`X-Api-Key`, `?key=`, cookie `api_key`)
/// — factorisee pour `router::api_not_found` (parite 401/404 des
/// chemins inconnus sous `/api`, que le fallback axum priverait du
/// middleware).
pub(crate) fn provided_key(req: &Request<axum::body::Body>) -> Option<String> {
    req.headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query_key(req.uri().query().unwrap_or("")))
        .or_else(|| cookie_key(req.headers().get("cookie")?.to_str().ok()?))
}

/// Extrait `key=` de la query string (premier gagnant, comme
/// `request.query.get("key")` aiohttp).
fn query_key(query: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("key=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Extrait `api_key=` de l'en-tete `Cookie`.
fn cookie_key(cookie_header: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("api_key=") {
            return Some(v.to_string());
        }
    }
    None
}
