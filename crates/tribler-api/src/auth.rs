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
//! du Python n'ont pas d'equivalent ici (ces routes n'existent pas).

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

    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query_key(req.uri().query().unwrap_or("")))
        .or_else(|| cookie_key(req.headers().get("cookie")?.to_str().ok()?));

    if provided.as_deref() == Some(expected) {
        next.run(req).await
    } else {
        ApiError::unauthorized().into_response()
    }
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
