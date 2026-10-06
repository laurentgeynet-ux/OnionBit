// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/messaging/*` — extension Rust (ADR-0011, etape 40).
//!
//! Pas de parite Python (Tribler n'a pas de messagerie e2e) : les
//! endpoints exposent le [`MessagingService`] — contacts
//! (consentement, blocage, retention), messages (historique borne,
//! envoi, suppression reelle) et un flux SSE dedie
//! `/api/messaging/events`.
//!
//! La messagerie etant `enable_messaging`-dependante, chaque
//! endpoint repond 404 `« messagerie desactivee »` quand le service
//! n'est pas demarre — jamais de reponse partielle.
//!
//! UI : l'application ne lit jamais le core directement — tout
//! passe par ces routes.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::stream::Stream;
use onionbit_core::services::messaging::{ContactState, MessagingEvent, MessagingService};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::error::ApiError;
use crate::state::AppState;

/// Borne par defaut de l'historique (`?limit=`).
const DEFAULT_HISTORY_LIMIT: u32 = 100;
/// Borne haute de l'historique — l'API ne sert jamais un flux
/// non borne.
const MAX_HISTORY_LIMIT: u32 = 500;

/// Service messagerie de la session, ou 404 si desactive.
fn svc(state: &AppState) -> Result<Arc<MessagingService>, ApiError> {
    state
        .session
        .ipv8()
        .and_then(|s| s.messaging.clone())
        .ok_or_else(|| ApiError::not_found("messagerie desactivee"))
}

/// `pk_bin` depuis son chemin hex (`{pk}`).
fn pk_hex(param: &str) -> Result<Vec<u8>, ApiError> {
    hex::decode(param).map_err(|_| ApiError::bad_request("public_key attendue en hex"))
}

/// `id` de message depuis son chemin hex.
fn id_hex(param: &str) -> Result<[u8; 16], ApiError> {
    let v = hex::decode(param).map_err(|_| ApiError::bad_request("id attendu en hex"))?;
    <[u8; 16]>::try_from(v.as_slice()).map_err(|_| ApiError::bad_request("id attendu : 16 octets"))
}

fn hexs(b: &[u8]) -> String {
    hex::encode(b)
}

fn state_str(s: ContactState) -> &'static str {
    match s {
        ContactState::Active => "active",
        ContactState::Pending => "pending",
        ContactState::Blocked => "blocked",
    }
}

/// `GET /api/messaging/stats` — identite locale + compteurs de
/// drops du demux.
pub async fn get_stats(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let stats: serde_json::Map<String, serde_json::Value> = svc
        .stats_snapshot()
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();
    Ok(Json(serde_json::json!({
        "public_key": hexs(&svc.public_key_bin()),
        "messaging_hash": hexs(&svc.own_messaging_hash()),
        "stats": stats,
    })))
}

/// `GET /api/messaging/contacts` — tous les contacts connus du
/// service avec leur etat de consentement et leur circuit lie.
pub async fn get_contacts(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let contacts: Vec<serde_json::Value> = svc
        .bound_contacts()
        .into_iter()
        .map(|(pk, cid)| {
            serde_json::json!({
                "public_key": hexs(&pk),
                "state": svc.contact_state(&pk).map(state_str).unwrap_or("unknown"),
                "circuit_id": cid,
                "alias": svc.contact_alias(&pk).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "contacts": contacts })))
}

/// `GET /api/messaging/contacts/pending` — demandes de
/// consentement en attente.
pub async fn get_pending(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let contacts: Vec<serde_json::Value> = svc
        .pending_contacts()
        .into_iter()
        .map(|(pk, since)| {
            serde_json::json!({
                "public_key": hexs(&pk),
                "pending_since_secs": since,
                "alias": svc.contact_alias(&pk).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "contacts": contacts })))
}

/// Corps `{public_key: "<hex>"}` des transitions de contact.
#[derive(serde::Deserialize)]
pub struct ConnectBody {
    /// Cle publique du contact (hex).
    pub public_key: String,
}

/// `POST /api/messaging/contacts/connect` — resout les points
/// d'introduction du contact puis lie un circuit e2e
/// (`resolve` DHT puis `connect`).
pub async fn post_connect(
    State(state): State<AppState>,
    Json(body): Json<ConnectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let pk = pk_hex(&body.public_key)?;
    let ips = svc.resolve(&pk).await?;
    let Some(ip) = ips.into_iter().next() else {
        return Err(ApiError::not_found(
            "aucun point d'introduction pour ce contact",
        ));
    };
    let cid = svc.connect(&pk, &ip).await?;
    Ok(Json(serde_json::json!({ "circuit_id": cid })))
}

/// `POST /api/messaging/contacts/{pk}/accept` — accorde le
/// consentement (`pending` -> `active`, trame `accept` au pair).
pub async fn post_accept(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.accept_contact(&pk_hex(&pk)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/contacts/{pk}/refuse` — refuse la demande
/// (trame `reject`, contact oublie).
pub async fn post_refuse(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.refuse_contact(&pk_hex(&pk)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/contacts/{pk}/block` — bloque le contact
/// (trames ignorees, swarm desarme, circuit detruit).
pub async fn post_block(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.block_contact(&pk_hex(&pk)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `DELETE /api/messaging/contacts/{pk}/block` — debloque.
pub async fn delete_block(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.unblock_contact(&pk_hex(&pk)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `DELETE /api/messaging/contacts/{pk}` — oublie le contact
/// (refus silencieux + suppression reelle de l'historique).
pub async fn delete_contact(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.refuse_contact(&pk_hex(&pk)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `?limit=` de l'historique.
#[derive(serde::Deserialize)]
pub struct HistoryQuery {
    /// Nombre max de messages (defaut 100, borne 500).
    pub limit: Option<u32>,
}

/// `GET /api/messaging/contacts/{pk}/messages` — historique borne
/// (le plus recent d'abord).
pub async fn get_messages(
    State(state): State<AppState>,
    Path(pk): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let limit = q
        .limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .min(MAX_HISTORY_LIMIT);
    let messages: Vec<serde_json::Value> = svc
        .history(&pk_hex(&pk)?, limit)?
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": hexs(&m.id),
                "direction": m.direction,
                "seq": m.seq,
                "ts": m.ts,
                "body": String::from_utf8_lossy(&m.body),
                "status": m.status,
                "created_at": m.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// Corps `{body: "<texte>"}` d'un envoi de message.
#[derive(serde::Deserialize)]
pub struct SendBody {
    /// Corps applicatif (UTF-8).
    pub body: String,
}

/// `POST /api/messaging/contacts/{pk}/messages` — envoie un
/// message. `404` si le contact est hors ligne (non livre,
/// enregistre `failed` dans l'historique — jamais de file).
pub async fn post_message(
    State(state): State<AppState>,
    Path(pk): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = svc(&state)?
        .send(&pk_hex(&pk)?, body.body.into_bytes())
        .await?;
    Ok(Json(serde_json::json!({ "id": hexs(&id) })))
}

/// `DELETE /api/messaging/messages/{id}` — suppression reelle.
pub async fn delete_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.delete_message(&id_hex(&id)?)?;
    Ok(Json(serde_json::json!({})))
}

/// Corps `{retention_secs, secure_delete}` de la retention.
#[derive(serde::Deserialize)]
pub struct RetentionBody {
    /// Retention des messages en secondes (`0` = conservation).
    pub retention_secs: u64,
    /// Zeroiser le corps avant suppression a l'expiration.
    #[serde(default)]
    pub secure_delete: bool,
}

/// Corps `{alias: "<texte>"}` du pseudonyme local.
#[derive(serde::Deserialize)]
pub struct AliasBody {
    /// Pseudonyme local (`""` = effacer — retour a la cle abregee).
    pub alias: String,
}

/// `POST /api/messaging/contacts/{pk}/alias` — pseudonyme local du
/// contact (affichage UI ; `""` efface).
pub async fn post_alias(
    State(state): State<AppState>,
    Path(pk): Path<String>,
    Json(body): Json<AliasBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.set_alias(&pk_hex(&pk)?, &body.alias)?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/contacts/{pk}/retention` — retention des
/// messages du contact.
pub async fn post_retention(
    State(state): State<AppState>,
    Path(pk): Path<String>,
    Json(body): Json<RetentionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.set_retention(&pk_hex(&pk)?, body.retention_secs, body.secure_delete)?;
    Ok(Json(serde_json::json!({})))
}

/// `MessagingEvent` → `(topic, json)` du flux SSE dedie.
fn event_to_sse(ev: &MessagingEvent) -> Option<(String, serde_json::Value)> {
    let (topic, kwargs) = match ev {
        MessagingEvent::Frame {
            contact,
            kind,
            id,
            body,
        } => (
            "messaging_frame",
            serde_json::json!({
                "contact": hexs(contact),
                "kind": format!("{kind:?}").to_lowercase(),
                "id": hexs(id),
                "body": String::from_utf8_lossy(body),
            }),
        ),
        MessagingEvent::Bound {
            contact,
            circuit_id,
        } => (
            "messaging_bound",
            serde_json::json!({"contact": hexs(contact), "circuit_id": circuit_id}),
        ),
        MessagingEvent::Pending { circuit_id } => (
            "messaging_pending",
            serde_json::json!({"circuit_id": circuit_id}),
        ),
        MessagingEvent::Consent {
            contact,
            circuit_id,
        } => (
            "messaging_consent",
            serde_json::json!({"contact": hexs(contact), "circuit_id": circuit_id}),
        ),
        MessagingEvent::Undeliverable { contact, id } => (
            "messaging_undeliverable",
            serde_json::json!({"contact": hexs(contact), "id": hexs(id)}),
        ),
        MessagingEvent::ContactAdded { contact } => (
            "messaging_contact",
            serde_json::json!({"contact": hexs(contact)}),
        ),
    };
    Some((topic.to_string(), kwargs))
}

/// `GET /api/messaging/events` — flux SSE dedie de la messagerie
/// (frames livrees, liaisons, consentements, non-livraisons).
/// 404 en JSON si la messagerie est desactivee.
pub async fn get_events(
    State(state): State<AppState>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = svc(&state)?.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(ev) => event_to_sse(&ev)
            .map(|(topic, kwargs)| Ok(Event::default().event(topic).data(kwargs.to_string()))),
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
