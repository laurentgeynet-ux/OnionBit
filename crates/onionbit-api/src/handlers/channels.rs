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

/// Cle de signature du canal personnel — 503 quand la stack IPv8
/// n'existe pas encore (identite non resolue, session invitee sans
/// ipv8, stealth sans overlay).
fn signing_key(
    state: &AppState,
) -> Result<onionbit_crypto::ipv8::keys::LibNaClSecretKey, ApiError> {
    state
        .session
        .ipv8()
        .map(|s| s.signing_key())
        .ok_or_else(|| ApiError::internal("stack ipv8 absente — canal personnel indisponible"))
}

/// Decode un info-hash hex (20 octets).
fn infohash_param(ih_hex: &str) -> Result<[u8; 20], ApiError> {
    let raw =
        hex::decode(ih_hex.trim()).map_err(|_| ApiError::bad_request("infohash hex attendu"))?;
    raw.as_slice()
        .try_into()
        .map_err(|_| ApiError::bad_request("infohash hex attendu (20 octets)"))
}

/// Corps de `PUT /api/channels/personal`.
#[derive(serde::Deserialize)]
pub struct PersonalChannelBody {
    /// Titre de la racine `COLLECTION_NODE` (renommage re-signe).
    pub title: String,
}

/// `PUT /api/channels/personal` — cree ou renomme la racine
/// `COLLECTION_NODE` (220) du canal personnel, signee par la cle de
/// session. Extension OnionBit (Tribler 8.x n'expose plus d'endpoint
/// de publication de canal).
pub async fn personal_set_title(
    State(state): State<AppState>,
    Json(body): Json<PersonalChannelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let key = signing_key(&state)?;
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title vide"));
    }
    let title = body.title.trim().to_string();
    let id = state
        .session
        .db()
        .call("channels.personal_title", move |c| {
            onionbit_core::channel_ops::set_title(c, &key, &title)
                .map_err(|e| onionbit_db::DbError::Corrupt(e.to_string()))
        })
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

/// `PUT /api/channels/personal/{infohash}/commit` — ajoute le
/// torrent au canal personnel (`CHANNEL_TORRENT` 400 signe,
/// `origin_id` = racine ; racine creee au premier commit).
pub async fn personal_commit(
    State(state): State<AppState>,
    Path(ih_hex): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let infohash = infohash_param(&ih_hex)?;
    let key = signing_key(&state)?;
    let added = state
        .session
        .db()
        .call("channels.personal_commit", move |c| {
            onionbit_core::channel_ops::commit(c, &key, &infohash)
                .map_err(|e| onionbit_db::DbError::Corrupt(e.to_string()))
        })
        .await?;
    match added {
        Some(id) => Ok(Json(serde_json::json!({ "added": true, "id": id }))),
        None => Err(ApiError::not_found("infohash inconnu de la base")),
    }
}

/// `DELETE /api/channels/personal/{infohash}` — retire le torrent
/// du canal personnel : la ligne devient pierre tombale `DELETED`
/// (500) signee, servie aux abonnes a la prochaine sync.
pub async fn personal_remove(
    State(state): State<AppState>,
    Path(ih_hex): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let infohash = infohash_param(&ih_hex)?;
    let key = signing_key(&state)?;
    let removed = state
        .session
        .db()
        .call("channels.personal_remove", move |c| {
            onionbit_core::channel_ops::remove(c, &key, &infohash)
                .map_err(|e| onionbit_db::DbError::Corrupt(e.to_string()))
        })
        .await?;
    if !removed {
        return Err(ApiError::not_found("entree absente du canal personnel"));
    }
    Ok(Json(serde_json::json!({ "removed": true })))
}
