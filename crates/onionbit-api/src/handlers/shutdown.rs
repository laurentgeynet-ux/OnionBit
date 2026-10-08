// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handler `/api/shutdown` — equivalent de
//! `tribler.core.restapi.shutdown_endpoint`.

use axum::extract::State;
use axum::Json;

use crate::state::AppState;

/// `PUT /api/shutdown` — demande l'arret de la session.
///
/// Comme le Python (`shutdown_request` → `events` notifie
/// `tribler_shutdown_started` puis l'arret est lance), la reponse part
/// avant l'arret effectif, qui s'execute en tache de fond.
pub async fn shutdown(State(state): State<AppState>) -> Json<serde_json::Value> {
    let session = state.session.clone();
    tokio::spawn(async move {
        session.stop().await;
    });
    // Termine aussi le processus : le graceful shutdown d'axum attend
    // la fin des reponses en vol (celle-ci comprise) puis le daemon
    // quitte. `stop()` est idempotent — l'appel ci-dessus et celui de
    // la sequence d'arret principale ne se dedoublent pas.
    if let Some(notify) = &state.shutdown_notify {
        notify.notify_one();
    }
    Json(serde_json::json!({ "shutdown": true }))
}
