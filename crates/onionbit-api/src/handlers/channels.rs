// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/channels` — equivalent de
//! `tribler.core.content_discovery.restapi.channels_endpoint`
//! (sous-ensemble ADR-0025 etape 97 : liste, contenu,
//! abonnement ; `copy` et le canal personnel arrivent a
//! l'etape 98).

use axum::extract::{Path, State};
use axum::Json;

use crate::error::ApiError;
use crate::state::AppState;

/// Decode une cle de canal hex (64 octets Ed25519).
fn channel_pk(pk_hex: &str) -> Result<Vec<u8>, ApiError> {
    let pk =
        hex::decode(pk_hex.trim()).map_err(|_| ApiError::bad_request("channel_pk hex attendu"))?;
    if pk.len() != 64 {
        return Err(ApiError::bad_request("channel_pk hex attendu (64 octets)"));
    }
    Ok(pk)
}

/// `GET /api/channels` — canaux suivis (`subscribed = 1`), un objet
/// par `(public_key, origin_id)` : `name` = titre de la racine si
/// connue, `num_entries` = entrees actives persistees.
pub async fn list_channels(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let channels = state
        .session
        .db()
        .call("channels.list", |c| {
            let subs = onionbit_db::channel::subscribed_channels(c)?;
            let mut out = Vec::with_capacity(subs.len());
            for (pk, origin_id) in subs {
                // Titre de la racine placeholder ou du vrai
                // `CHANNEL_NODE` (id_ == origin_id).
                let root =
                    onionbit_db::channel::get_by_pk_id(c, &pk, origin_id)?.unwrap_or_default();
                out.push(serde_json::json!({
                    "public_key": hex::encode(&pk),
                    "id": origin_id,
                    "name": root.title,
                    "subscribed": true,
                    "num_entries": onionbit_db::channel::count_channel_entries(c, &pk)?,
                }));
            }
            Ok(out)
        })
        .await?;
    Ok(Json(serde_json::json!({ "channels": channels })))
}

/// `GET /api/channels/{pk}/{id}` — contenu persiste du canal
/// (entrees `CHANNEL_TORRENT` au format torrent de
/// `/api/metadata`, tri timestamp desc).
pub async fn channel_contents(
    State(state): State<AppState>,
    Path((pk_hex, id)): Path<(String, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pk = channel_pk(&pk_hex)?;
    let rows = state
        .session
        .db()
        .call("channels.contents", move |c| {
            onionbit_db::channel::select_entries(
                c,
                &onionbit_db::channel::SelectParams {
                    channel_pk: Some(pk.clone()),
                    origin_id: Some(id),
                    sort_by: Some("timestamp".into()),
                    sort_desc: true,
                    first: 1,
                    last: Some(100),
                    ..Default::default()
                },
            )
        })
        .await?;
    let results: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| crate::handlers::metadata::row_json_with_trackers(r, Vec::new()))
        .collect();
    Ok(Json(serde_json::json!({ "results": results })))
}

/// `PUT /api/channels/{pk}/{id}/subscribe` — suit le canal
/// (`channels_endpoint` Python) : cree la racine placeholder si
/// inconnue, la tache `channel_sync` le tirera des pairs.
pub async fn subscribe(
    State(state): State<AppState>,
    Path((pk_hex, id)): Path<(String, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pk = channel_pk(&pk_hex)?;
    state
        .session
        .db()
        .call("channels.subscribe", move |c| {
            onionbit_db::channel::set_subscribed(c, &pk, id, true)
        })
        .await?;
    Ok(Json(serde_json::json!({ "subscribed": true })))
}

/// `DELETE /api/channels/{pk}/{id}/subscribe` — desabonnement
/// (les entrees persistees restent, la sync s'arrete au prochain
/// tick).
pub async fn unsubscribe(
    State(state): State<AppState>,
    Path((pk_hex, id)): Path<(String, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pk = channel_pk(&pk_hex)?;
    state
        .session
        .db()
        .call("channels.unsubscribe", move |c| {
            onionbit_db::channel::set_subscribed(c, &pk, id, false)
        })
        .await?;
    Ok(Json(serde_json::json!({ "subscribed": false })))
}
