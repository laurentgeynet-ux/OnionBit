// This file is part of OnionBit - a Rust port of the Tribler daemon.
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
//! - `Cache-Control: no-cache` partout : les fichiers sont revalidés
//!   par ETag/`Last-Modified` de `ServeDir` (304) — `index.html` ne
//!   doit jamais être servi périmé après une mise à jour ;
//! - `X-Content-Type-Options: nosniff`.

use std::path::Path;

use axum::http::{header, HeaderValue};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

/// Routeur des statiques web — monté en `fallback` du routeur racine
/// (hors `/api`), donc jamais soumis à `api_key_auth` comme les
/// exemptions `/ui`/`/static` du middleware Python.
pub fn router(dir: &Path) -> Router<AppState> {
    let serve = ServeDir::new(dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(dir.join("index.html")));
    Router::new()
        .fallback_service(serve)
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
}
