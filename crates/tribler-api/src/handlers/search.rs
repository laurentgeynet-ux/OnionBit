//! Handler `/api/search` — equivalent de
//! `tribler.core.content_discovery.restapi.search_endpoint`
//! (`remote_search` : `send_search_request` vers un echantillon
//! aleatoire de pairs de la community ; les resultats reviennent via
//! les SelectResponses integrees par `ContentProvider::
//! process_select_response` puis notifies `remote_query_results`).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `PUT /api/search/remote?fts_text=...` — recherche distante sur la
/// community content-discovery. Parametres = `RemoteQueryParameters`
/// + `MetadataParameters` Python ; reponse
/// `{"request_uuid", "peers": [mids]}` comme Python.
#[derive(Debug, Deserialize)]
pub struct RemoteSearchQuery {
    /// Texte de recherche (obligatoire, converti en `txt_filter` FTS).
    pub fts_text: Option<String>,
    /// Filtre additionnel (`filter` Python ajoute au texte).
    pub filter: Option<String>,
    /// `first` (defaut Python 1).
    pub first: Option<u64>,
    /// `last` (defaut Python 50).
    pub last: Option<u64>,
    /// `sort_by` (relaye tel quel dans le JSON du select).
    pub sort_by: Option<String>,
    /// `sort_desc` (defaut Python true).
    pub sort_desc: Option<bool>,
    /// `hide_xxx` (defaut Python false).
    pub hide_xxx: Option<bool>,
    /// `category` (filtre de tags Python).
    pub category: Option<String>,
    /// `metadata_type` (chaine -> `[int]` cote Python
    /// `convert_to_json`).
    pub metadata_type: Option<String>,
    /// `channel_pk` hex (requiert `origin_id` cote Python).
    pub channel_pk: Option<String>,
    /// `origin_id`.
    pub origin_id: Option<i64>,
    /// `max_rowid` (pagination pushback).
    pub max_rowid: Option<i64>,
    /// `tags` (liste separee par virgules — Python accepte le
    /// parametre repete `tags`).
    pub tags: Option<String>,
}

/// `parse_bool` Python (`json.loads`).
fn parse_bool_flag(v: Option<bool>) -> bool {
    v.unwrap_or(false)
}

pub async fn remote_search(
    State(state): State<AppState>,
    Query(q): Query<RemoteSearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut query_text = q
        .fts_text
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("fts_text parameter missing"))?;
    if let Some(f) = &q.filter {
        query_text.push(' ');
        query_text.push_str(f);
    }
    let stack = state.session.ipv8().ok_or_else(|| {
        ApiError::bad_request("ipv8 stack inactive (recherche distante impossible)")
    })?;

    // `sanitize_parameters` + `convert_to_json` Python : le JSON du
    // `RemoteSelectPayload` transporte les kwargs de la requete.
    let mut json = serde_json::json!({
        "first": q.first.unwrap_or(1),
        "last": q.last.unwrap_or(50),
        "sort_desc": q.sort_desc.unwrap_or(true),
        "hide_xxx": parse_bool_flag(q.hide_xxx),
    });
    if let Some(fts) = tribler_core::queries::to_fts_query(&query_text) {
        json["txt_filter"] = serde_json::json!(fts);
    }
    if let Some(v) = &q.sort_by {
        json["sort_by"] = serde_json::json!(v);
    }
    if let Some(v) = &q.category {
        json["category"] = serde_json::json!(v);
    }
    if let Some(mt) = &q.metadata_type {
        // `convert_to_json` : chaine -> liste d'entiers.
        json["metadata_type"] = serde_json::json!([mt]);
    }
    if let Some(v) = &q.channel_pk {
        json["channel_pk"] = serde_json::json!(v);
    }
    if let Some(v) = q.origin_id {
        json["origin_id"] = serde_json::json!(v);
    }
    if let Some(v) = q.max_rowid {
        json["max_rowid"] = serde_json::json!(v);
    }
    if let Some(v) = &q.tags {
        json["tags"] = serde_json::json!(v.split(',').map(str::trim).collect::<Vec<_>>());
    }
    let body = serde_json::to_vec(&json).map_err(|e| ApiError::internal(e.to_string()))?;

    // `send_search_request` Python : echantillon aleatoire de
    // `max_query_peers` pairs ; `notify_gui` notifie
    // `remote_query_results` (query, results, uuid, mid du pair) a
    // chaque paquet de reponse.
    let uuid = uuid::Uuid::new_v4().to_string();
    let notifier = state.session.notifier().clone();
    let (uuid_c, query_c) = (uuid.clone(), query_text.clone());
    let cb: tribler_ipv8::content_discovery::SelectCallback =
        std::sync::Arc::new(move |mid, results| {
            notifier.notify(tribler_core::Notification::RemoteQueryResults {
                query: query_c.clone(),
                results,
                uuid: uuid_c.clone(),
                peer: hex::encode(mid),
            });
        });
    // `content_discovery_community/enabled=false` : pas de community
    // -> aucun pair interroge (la recherche locale REST reste
    // disponible via `/api/metadata/search/local`).
    let queried = match &stack.content_discovery {
        Some(cd) => cd.send_search_request(body, cb).await,
        None => Vec::new(),
    };
    tracing::info!(
        %uuid,
        query = %query_text,
        queried = queried.len(),
        "recherche distante emise"
    );
    Ok(Json(serde_json::json!({
        "request_uuid": uuid,
        "peers": queried,
    })))
}
