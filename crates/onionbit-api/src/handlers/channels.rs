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
                // ADR-0029 : adresse `.onion` du createur si
                // associee (liaison explicite — `peer_onions`).
                let onion = onionbit_db::peer_onion::get_peer_onion(c, &pk)?;
                let mut planes = vec!["onionbit"];
                if onion.is_some() {
                    planes.push("onion");
                }
                out.push(serde_json::json!({
                    "public_key": hex::encode(&pk),
                    "id": origin_id,
                    "name": root.title,
                    "subscribed": true,
                    "num_entries": onionbit_db::channel::count_channel_entries(c, &pk)?,
                    "onion": onion.as_ref().map(|o| o.onion_host.clone()),
                    "onion_verified": onion.map(|o| o.sig_checked).unwrap_or(false),
                    "planes": planes,
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

/// Corps `{address, signature?}` de la liaison `.onion` d'un canal
/// (ADR-0029 etape 123) — la cle du canal est la cle d'identite de
/// son createur : la table `peer_onions` est partagee avec les
/// contacts messagerie, liaison **explicite** jamais inferee.
#[derive(serde::Deserialize)]
pub struct ChannelOnionBody {
    /// Adresse `.onion` du createur (`onion:<hote>` ou hote v3 nu).
    pub address: String,
    /// Signature `onionbit/onion-bind/v1` hex optionnelle — quand
    /// fournie, elle DOIT verifier sous la cle du canal.
    pub signature: Option<String>,
}

/// `PUT /api/channels/{pk}/onion` — associe l'adresse `.onion` du
/// createur au canal (servira le catalogue mdblob en HTTP sur ce
/// service, etape 126) ; l'hote rejoint l'allowlist etendue Tor.
/// `400` si l'adresse est invalide ou la signature ne verifie pas.
pub async fn put_channel_onion(
    State(state): State<AppState>,
    Path(pk_hex): Path<String>,
    Json(body): Json<ChannelOnionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pk = channel_pk(&pk_hex)?;
    let host = body
        .address
        .trim()
        .strip_prefix("onion:")
        .unwrap_or(body.address.trim());
    let sig = match &body.signature {
        Some(s) => Some(
            hex::decode(s.trim())
                .map_err(|_| ApiError::bad_request("signature attendue en hex"))?,
        ),
        None => None,
    };
    let row = state
        .session
        .set_peer_onion(&pk, host, sig.as_deref())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "onion": row.onion_host,
        "sig_checked": row.sig_checked,
    })))
}

/// `DELETE /api/channels/{pk}/onion` — retire l'association (le
/// canal redevient joignable au seul plan OnionBit cote UI).
pub async fn delete_channel_onion(
    State(state): State<AppState>,
    Path(pk_hex): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let removed = state
        .session
        .remove_peer_onion(&channel_pk(&pk_hex)?)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "removed": removed })))
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
