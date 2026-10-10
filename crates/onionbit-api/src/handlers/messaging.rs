// This file is part of OnionBit.
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
use onionbit_core::services::messaging::{
    ContactState, LinkState, MessagingEvent, MessagingService,
};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::error::ApiError;
use crate::state::AppState;
use onionbit_core::config::StorageArea;
use onionbit_core::session::attach::AttachTarget;

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

fn link_str(l: LinkState) -> &'static str {
    match l {
        LinkState::Bound => "bound",
        LinkState::Connecting => "connecting",
        LinkState::Failed => "failed",
        LinkState::None => "none",
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
                "link": link_str(svc.link_state(&pk)),
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
                "link": link_str(svc.link_state(&pk)),
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
/// (`connect_peer` : `resolve` DHT puis `connect`, avec suivi de
/// l'etat de liaison expose dans `GET /contacts` — champ `link`).
pub async fn post_connect(
    State(state): State<AppState>,
    Json(body): Json<ConnectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let pk = pk_hex(&body.public_key)?;
    let cid = svc.connect_peer(&pk).await?;
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

/// `GET /api/messaging/vault/export` — coffre de contacts chiffre
/// pour notre propre identite (`OBV1` + AEAD pairbox, cle derivee
/// du DH `crypt_pk x crypt_sk`) : `{pk, alias, state}` par contact.
/// Portable entre devices partageant la meme identite (ADR-0016) ;
/// illisible sans la cle privee — pseudonymes inclus, contrairement
/// aux auto-attestations `identity` (publiques par design).
pub async fn get_vault_export(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let blob = svc(&state)?.export_vault();
    Ok(Json(serde_json::json!({ "vault": hexs(&blob) })))
}

/// Corps `{vault: "<hex>"}` de `POST /api/messaging/vault/import`.
#[derive(serde::Deserialize)]
pub struct VaultImportBody {
    /// Blob `OBV1…` hex tel que rendu par l'export.
    pub vault: String,
}

/// `POST /api/messaging/vault/import` — dechiffre le blob pour notre
/// identite et restaure les contacts inconnus en `Active` (ou
/// `blocked` si tel etait leur etat exporte) + alias. Renvoie
/// `{restored: n}`.
pub async fn post_vault_import(
    State(state): State<AppState>,
    Json(body): Json<VaultImportBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let blob = hex::decode(&body.vault).map_err(|_| ApiError::bad_request("vault hex attendu"))?;
    let restored = svc(&state)?.import_vault(&blob)?;
    Ok(Json(serde_json::json!({ "restored": restored })))
}

/// Delai d'attente d'une `VAULT_RESP` pour `/vault/restore` — un
/// pont muet ne doit pas bloquer la requete indefiniment.
const VAULT_PULL_TIMEOUT_SECS: u64 = 10;

/// Stack IPv8 de la session (pull store — ADR-0026), ou 404.
fn ipv8(state: &AppState) -> Result<Arc<onionbit_core::ipv8_stack::Ipv8Stack>, ApiError> {
    state
        .session
        .ipv8()
        .ok_or_else(|| ApiError::not_found("ipv8 desactive"))
}

/// `POST /api/messaging/vault/replicate` — `VAULT_PUT` du coffre
/// `OBV1` courant (contacts chiffres pour notre identite) vers
/// **tous** les ponts `CAP_PULL_STORE` connus : la replication
/// multi-ponts est la resilience (remplacement a chaque pont).
/// Renvoie `{replicated: n}` ; 404 sans pont disponible.
pub async fn post_vault_replicate(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let blob = svc(&state)?.export_vault();
    let sent = ipv8(&state)?.vault_push(&blob).await;
    if sent == 0 {
        return Err(ApiError::not_found("aucun pont pull_store disponible"));
    }
    Ok(Json(serde_json::json!({ "replicated": sent })))
}

/// `POST /api/messaging/vault/restore` — `VAULT_GET` sur un pont
/// `CAP_PULL_STORE` puis [`import_vault`] du blob rendu. Renvoie
/// `{restored: n}` ; 404 si aucun pont ou coffre absent (`not_found`
/// du store).
pub async fn post_vault_restore(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(blob) = ipv8(&state)?
        .vault_pull(std::time::Duration::from_secs(VAULT_PULL_TIMEOUT_SECS))
        .await
    else {
        return Err(ApiError::not_found("aucun coffre chez les ponts"));
    };
    let restored = svc(&state)?.import_vault(&blob)?;
    Ok(Json(serde_json::json!({ "restored": restored })))
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
        MessagingEvent::Link { contact, link } => (
            "messaging_link",
            serde_json::json!({"contact": hexs(contact), "link": link_str(*link)}),
        ),
        MessagingEvent::Conv {
            conv,
            contact,
            kind,
            id,
            body,
        } => (
            "messaging_conv",
            serde_json::json!({
                "conv_id": hexs(conv),
                "contact": hexs(contact),
                "kind": format!("{kind:?}").to_lowercase(),
                "id": hexs(id),
                "body": String::from_utf8_lossy(body),
            }),
        ),
        MessagingEvent::GroupInvite { conv, name, by } => (
            "messaging_group_invite",
            serde_json::json!({
                "conv_id": hexs(conv),
                "name": name,
                "by": hexs(by),
            }),
        ),
        MessagingEvent::Attach {
            conv,
            contact,
            attach_id,
            ih,
            name,
            size,
        } => (
            "messaging_attach",
            serde_json::json!({
                "conv_id": hexs(conv),
                "contact": hexs(contact),
                "attach_id": hexs(attach_id),
                "infohash": hexs(ih),
                "name": name,
                "size": size,
            }),
        ),
    };
    Some((topic.to_string(), kwargs))
}

// ── ADR-0019 : conversations, groupes, pieces jointes ───────────

/// `conv_id` depuis son chemin hex (16 octets).
fn conv_hex(param: &str) -> Result<[u8; 16], ApiError> {
    id_hex(param)
}

/// `GET /api/messaging/conversations` — toutes les conversations
/// (directes + groupes) avec non-lus et dernier horodatage.
pub async fn get_conversations(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let conversations: Vec<serde_json::Value> = svc
        .conversation_list()?
        .into_iter()
        .map(|c| {
            let peer = svc.conv_peer(&c.row.conv_id.clone().try_into().unwrap_or_default());
            serde_json::json!({
                "conv_id": hexs(&c.row.conv_id),
                "kind": c.row.kind,
                "name": c.row.name,
                "state": c.row.state,
                "created_at": c.row.created_at,
                "unread": c.unread,
                "last_ts": c.last_ts,
                "peer": peer.as_ref().map(|p| hexs(p)),
                "alias": peer.and_then(|p| svc.contact_alias(&p)).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "conversations": conversations })))
}

/// `GET /api/messaging/conversations/direct/{pk}` — `conv_id`
/// deterministe de la conversation directe avec `pk` (la conv peut
/// ne pas encore exister en base — derivee pure).
pub async fn get_direct_conversation(
    State(state): State<AppState>,
    Path(pk): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let conv = svc.direct_conv(&pk_hex(&pk)?);
    Ok(Json(serde_json::json!({ "conv_id": hexs(&conv) })))
}

fn conv_json_conv(conv: &[u8; 16], m: &onionbit_db::messaging::MsgMessageRow) -> serde_json::Value {
    let _ = conv;
    serde_json::json!({
        "id": hexs(&m.id),
        "direction": m.direction,
        "seq": m.seq,
        "ts": m.ts,
        "body": String::from_utf8_lossy(&m.body),
        "status": m.status,
        "created_at": m.created_at,
        "author_pk": m.author_pk.as_ref().map(|p| hexs(p)),
        "mid": m.mid.as_ref().map(|p| hexs(p)),
    })
}

/// `GET /api/messaging/conversations/{conv}/messages?limit=` —
/// historique borne de la conversation (le plus recent d'abord).
pub async fn get_conv_messages(
    State(state): State<AppState>,
    Path(conv): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let conv = conv_hex(&conv)?;
    let limit = q
        .limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .min(MAX_HISTORY_LIMIT);
    let messages: Vec<serde_json::Value> = svc
        .conversation_messages(&conv, limit)?
        .into_iter()
        .map(|m| conv_json_conv(&conv, &m))
        .collect();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// `POST /api/messaging/conversations/{conv}/messages` — envoie dans
/// la conversation : `group_send` en groupe, `send` au contact de la
/// conv directe.
pub async fn post_conv_message(
    State(state): State<AppState>,
    Path(conv): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let conv = conv_hex(&conv)?;
    let id = if let Some(pk) = svc.conv_peer(&conv) {
        svc.send(&pk, body.body.into_bytes()).await?
    } else {
        svc.group_send(&conv, body.body.into_bytes()).await?
    };
    Ok(Json(serde_json::json!({ "id": hexs(&id) })))
}

/// `POST /api/messaging/conversations/{conv}/read` — marque lu.
pub async fn post_conv_read(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.conversation_mark_read(&conv_hex(&conv)?)?;
    Ok(Json(serde_json::json!({})))
}

/// `DELETE /api/messaging/conversations/{conv}` — suppression reelle
/// (cascade : messages, roster, livraisons, pieces jointes).
pub async fn delete_conversation(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.conversation_delete(&conv_hex(&conv)?)?;
    Ok(Json(serde_json::json!({})))
}

/// Corps `{name, members:[<pk hex>]}` de `POST /messaging/groups`.
#[derive(serde::Deserialize)]
pub struct GroupCreateBody {
    /// Nom affiche du groupe.
    pub name: String,
    /// `pk_bin` hex des membres invites (contacts actifs requis).
    pub members: Vec<String>,
}

/// `POST /api/messaging/groups` — cree un groupe : conv aleatoire +
/// `gctl invite` vers chaque contact actif.
pub async fn post_group(
    State(state): State<AppState>,
    Json(body): Json<GroupCreateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let members: Result<Vec<Vec<u8>>, ApiError> = body.members.iter().map(|m| pk_hex(m)).collect();
    let conv = svc.group_create(&body.name, &members?).await?;
    Ok(Json(serde_json::json!({ "conv_id": hexs(&conv) })))
}

/// `GET /api/messaging/groups/{conv}/members` — roster du groupe.
pub async fn get_group_members(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let members: Vec<serde_json::Value> = svc
        .conversation_members(&conv_hex(&conv)?)?
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "member_pk": hexs(&m.member_pk),
                "added_by": hexs(&m.added_by),
                "state": m.state,
                "joined_at": m.joined_at,
                "alias": svc.contact_alias(&m.member_pk).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "members": members })))
}

/// `POST /api/messaging/groups/{conv}/invite` — invite un contact
/// actif (emission `gctl invite` + roster additif).
pub async fn post_group_invite(
    State(state): State<AppState>,
    Path(conv): Path<String>,
    Json(body): Json<ConnectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?
        .group_invite(&conv_hex(&conv)?, &pk_hex(&body.public_key)?)
        .await?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/groups/{conv}/accept` — rejoint (`join` +
/// `active`).
pub async fn post_group_accept(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.group_accept(&conv_hex(&conv)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/groups/{conv}/decline` — decline
/// l'invitation (conv `left`, `gctl leave` a l'invitant).
pub async fn post_group_decline(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.group_decline(&conv_hex(&conv)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// `POST /api/messaging/groups/{conv}/leave` — quitte le groupe
/// (conv `left`, `gctl leave` aux membres).
pub async fn post_group_leave(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    svc(&state)?.group_leave(&conv_hex(&conv)?).await?;
    Ok(Json(serde_json::json!({})))
}

/// Corps `{path, name?}` de `POST /messaging/uploads` (variante
/// JSON — le fichier reste sur la machine du daemon).
#[derive(serde::Deserialize)]
pub struct UploadPathBody {
    /// Chemin local du fichier (hors `@private`).
    pub path: String,
    /// Nom affiche (defaut : basename du fichier).
    pub name: Option<String>,
}

/// `POST /api/messaging/uploads` — stage un fichier sous
/// `@state/messaging/uploads/<id>/` (TTL `upload_ttl`, bornes
/// `attach_max_bytes`/`attach_stage_max_bytes`, jamais sous
/// `data/public`).
///
/// Deux formes :
/// - JSON `{path, name?}` : fichier local du daemon (hors
///   `@private` — refuse).
/// - Octets bruts + `?name=` : envoi direct (drag & drop UI, web) —
///   lecture bornee par `attach_max_bytes`.
pub async fn post_upload(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<UploadQuery>,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let info = if content_type.contains("json") {
        let bytes = axum::body::to_bytes(body, 1 << 20)
            .await
            .map_err(|_| ApiError::bad_request("corps JSON trop grand"))?;
        let req: UploadPathBody = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::bad_request("corps JSON invalide"))?;
        state
            .session
            .stage_upload_path(std::path::Path::new(&req.path), req.name)?
    } else {
        let name = q
            .name
            .ok_or_else(|| ApiError::bad_request("parametre name manquant"))?;
        // `to_bytes` borne la lecture — `DefaultBodyLimit` global
        // desactive sur cette route (cap = `attach_max_bytes`).
        let cap = svc.config().attach_max_bytes as usize;
        let bytes = axum::body::to_bytes(body, cap)
            .await
            .map_err(|_| ApiError::bad_request("corps hors borne attach_max_bytes"))?;
        state.session.stage_upload_bytes(&name, &bytes)?
    };
    Ok(Json(serde_json::json!({
        "upload_id": hexs(&info.upload_id),
        "name": info.name,
        "size": info.size,
    })))
}

/// `?name=` de `POST /messaging/uploads` (forme octets).
#[derive(serde::Deserialize)]
pub struct UploadQuery {
    /// Nom affiche du fichier envoye en octets.
    pub name: Option<String>,
}

/// Corps `{upload_id?|path?, name?}` de
/// `POST /conversations/{conv}/attachments`.
#[derive(serde::Deserialize)]
pub struct AttachSendBody {
    /// `upload_id` d'un fichier stage par `POST /uploads`.
    pub upload_id: Option<String>,
    /// Chemin local direct (hors `@private`) — alternative a
    /// `upload_id`.
    pub path: Option<String>,
    /// Nom affiche (defaut : basename du fichier).
    pub name: Option<String>,
}

/// `POST /api/messaging/conversations/{conv}/attachments` — offre la
/// piece jointe stagee (ou le `{path}` direct) a la conversation :
/// torrent ephemere sale + seed anonyme + trames `attach`.
pub async fn post_conv_attach(
    State(state): State<AppState>,
    Path(conv): Path<String>,
    Json(body): Json<AttachSendBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = svc(&state)?;
    let conv = conv_hex(&conv)?;
    let source = match (&body.upload_id, &body.path) {
        (Some(id), None) => state.session.upload_path(
            &hex::decode(id).map_err(|_| ApiError::bad_request("upload_id attendu en hex"))?,
        )?,
        (None, Some(p)) => std::path::PathBuf::from(p),
        _ => return Err(ApiError::bad_request("upload_id ou path attendu (un seul)")),
    };
    let target = if let Some(pk) = svc.conv_peer(&conv) {
        AttachTarget::Contact(pk)
    } else {
        AttachTarget::Group(conv)
    };
    let offer = state
        .session
        .attach_offer(target, &source, body.name)
        .await?;
    Ok(Json(serde_json::json!({
        "attach_id": hexs(&offer.attach_id),
        "conv_id": hexs(&offer.conv),
        "infohash": hexs(&offer.ih),
        "name": offer.name,
        "size": offer.size,
        "sent": offer.sent,
    })))
}

/// `GET /api/messaging/conversations/{conv}/attachments` — offres et
/// receptions de la conversation.
pub async fn get_conv_attachments(
    State(state): State<AppState>,
    Path(conv): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _svc = svc(&state)?;
    let attachments: Vec<serde_json::Value> = state
        .session
        .attach_list(Some(conv_hex(&conv)?))?
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "attach_id": hexs(&a.attach_id),
                "conv_id": hexs(&a.conv_id),
                "infohash": hexs(&a.ih),
                "name": a.name,
                "size": a.size,
                "role": a.role,
                "state": a.state,
                "created_at": a.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "attachments": attachments })))
}

/// Corps `{area?, dir?}` de `POST /attachments/{id}/accept` —
/// grammaire `destination {area, dir?}` d'ADR-0018 : `dir` est un
/// spec portable (`@private/downloads`, `@public/...`) ou chemin
/// externe, resolu par `resolve_input`.
#[derive(serde::Deserialize)]
pub struct AttachAcceptBody {
    /// `"public"` ou `"private"` (defaut `attach_area` du service).
    pub area: Option<String>,
    /// `dir` destination (spec `@…` ou chemin — prive : seuls
    /// `temp`/`downloads` sont admis).
    pub dir: Option<String>,
}

/// `POST /api/messaging/attachments/{id}/accept` — accepte l'offre :
/// download anonyme de l'infohash sale **deporte en tache** (la
/// resolution BEP 9 peut durer en lane anonyme — meme deport que
/// `PUT /downloads`, l'offre passe `accepted` immediatement puis
/// `downloading` a la materialisation). `409 identity_locked` si la
/// zone privee visee est verrouillee.
pub async fn post_attach_accept(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AttachAcceptBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _svc = svc(&state)?;
    let area = match body.area.as_deref() {
        None | Some("") => None,
        Some("public") => Some(StorageArea::Public),
        Some("private") => Some(StorageArea::Private),
        Some(_) => return Err(ApiError::bad_request("area attendue : public|private")),
    };
    // Garde explicite `identity_locked` (409) avant tout travail —
    // la session renverrait la meme erreur en `InvalidState`, mais
    // le statut 409 est le contrat ADR-0018.
    if area == Some(StorageArea::Private) && state.session.private_area_state() == "locked" {
        return Err(ApiError::conflict("identity_locked"));
    }
    let destination = match &body.dir {
        None => None,
        Some(d) => Some(
            state
                .session
                .paths()
                .resolve_input(std::path::Path::new(d))
                .map_err(|e| ApiError::bad_request(e.to_string()))?,
        ),
    };
    let acc = state
        .session
        .attach_accept(&id_hex(&id)?, destination, area)
        .await?;
    Ok(Json(serde_json::json!({
        "infohash": acc.infohash,
        "name": acc.name,
    })))
}

/// `POST /api/messaging/attachments/{id}/decline` — refuse l'offre
/// (`offered` → `declined`, aucun telechargement lance).
pub async fn post_attach_decline(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _svc = svc(&state)?;
    state.session.attach_decline(&id_hex(&id)?)?;
    Ok(Json(serde_json::json!({})))
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
