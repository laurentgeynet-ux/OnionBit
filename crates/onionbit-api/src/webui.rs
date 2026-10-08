// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Service des statiques de l'interface web Flutter.
//!
//! L'UI compilée (`app/build/web`) est servie en same-origin par le
//! daemon : le navigateur obtient l'interface à `http://127.0.0.1:<p>/`
//! et parle à `/api/*` sans CORS ni configuration. C'est le modèle
//! des exemptions `/ui` et `/static` de l'`ApiKeyMiddleware` Python —
//! ces chemins ne portent pas la clé, **toutes les routes `/api`
//! restent derrière `api_key_auth`**.
//!
//! - `GET /` et `GET /<fichier existant>` : contenu du dossier ;
//! - tout autre chemin hors `/api` : repli `index.html` (routes
//!   `go_router` résolues côté client) ;
//! - **auto-connexion** (`api/web_ui_inject_key`, défaut `true`) :
//!   l'`index.html` servi embarque `<meta name="onionbit-api-key"
//!   content="…">` — le navigateur lisant la page (même origine
//!   loopback) récupère la clé comme la GUI desktop la lit dans
//!   `configuration.json`, sans saisie ni `?key=` ;
//! - `Cache-Control: no-cache` partout : les fichiers sont revalidés
//!   par ETag/`Last-Modified` de `ServeDir` (304) — `index.html` ne
//!   doit jamais être servi périmé après une mise à jour ;
//! - `X-Content-Type-Options: nosniff`.

use std::path::Path;

use axum::body::Bytes;
use axum::handler::HandlerWithoutStateExt;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

/// Routeur des statiques web — monté en `fallback` du routeur racine
/// (hors `/api`), donc jamais soumis à `api_key_auth` comme les
/// exemptions `/ui`/`/static` du middleware Python.
///
/// `inject_key` (`api/web_ui_inject_key`) injecte `api_key` dans
/// l'`index.html` servi pour que l'UI web se connecte seule — sans
/// danger hors loopback : le daemon ne bind que sur 127.0.0.1 et la
/// same-origin policy empêche un autre site de lire la réponse.
pub fn router(dir: &Path, api_key: Option<&str>, inject_key: bool) -> Router<AppState> {
    let app = match index_body(dir, api_key, inject_key) {
        Some(body) => {
            // `index.html` injecté : route explicite obligatoire —
            // le fichier brut existe sur disque et `ServeDir` le
            // servirait sans la meta. `/` et les routes inconnues
            // tombent dans le repli → même contenu injecté (SPA).
            let fallback = {
                let body = body.clone();
                (move || {
                    let body = body.clone();
                    async move { index_response(body) }
                })
                .into_service()
            };
            let route_body = body.clone();
            Router::new()
                .route(
                    "/index.html",
                    get(move || {
                        let body = route_body.clone();
                        async move { index_response(body) }
                    }),
                )
                .fallback_service(
                    ServeDir::new(dir)
                        // Pas d'index automatique : `/` doit passer
                        // par le repli qui sert la version injectée.
                        .append_index_html_on_directories(false)
                        .fallback(fallback),
                )
        }
        // `index.html` illisible (course rare avec la vérification
        // `is_file` du daemon) : service brut sans injection.
        None => Router::new().fallback_service(
            ServeDir::new(dir)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(dir.join("index.html"))),
        ),
    };
    app.layer(SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    ))
    .layer(SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ))
}

/// Contenu d'`index.html` (mémoire) avec la clé API injectée en meta
/// quand `inject_key` — la clé est filtrée à `[A-Za-z0-9_-]` pour
/// rester un attribut HTML sain. `None` si le fichier est illisible.
fn index_body(dir: &Path, api_key: Option<&str>, inject_key: bool) -> Option<Bytes> {
    let raw = std::fs::read(dir.join("index.html")).ok()?;
    let html = String::from_utf8(raw).ok()?;
    let key = api_key.filter(|k| {
        !k.is_empty()
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    let html = match (inject_key, key) {
        (true, Some(k)) => inject_meta(&html, k),
        _ => html,
    };
    Some(Bytes::from(html))
}

/// Insère `<meta name="onionbit-api-key">` avant `</head>`, sinon
/// après la balise `<head …>` ouvrante, sinon en tête du document
/// (un `meta` en tout début de fichier reste interprété).
fn inject_meta(html: &str, key: &str) -> String {
    let meta = format!("<meta name=\"onionbit-api-key\" content=\"{key}\">");
    if let Some(pos) = html.find("</head>") {
        format!("{}    {meta}\n{}", &html[..pos], &html[pos..])
    } else if let Some(pos) = html
        .find("<head")
        .and_then(|p| html[p..].find('>').map(|e| p + e + 1))
    {
        format!("{}\n    {meta}{}", &html[..pos], &html[pos..])
    } else {
        format!("{meta}\n{html}")
    }
}

/// Réponse `text/html` du contenu `index.html` injecté (partagée par
/// la route `/index.html` et le repli SPA).
fn index_response(body: Bytes) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}
