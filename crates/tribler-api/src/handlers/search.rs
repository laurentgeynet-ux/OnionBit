//! Handler `/api/search` — equivalent de
//! `tribler.core.content_discovery.restapi.search_endpoint`
//! (`remote_search` : RemoteSelect vers les pairs de la community ;
//! les resultats reviennent via les SelectResponses integrees a la
//! base par `ContentProvider::process_select_response`).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `PUT /api/search/remote?fts_text=...&metadata_type=...` —
/// recherche distante sur la community content-discovery.
/// Reponse : `{"request_uuid", "peers": [mids]}` comme Python.
#[derive(Debug, Deserialize)]
pub struct RemoteSearchQuery {
    /// Texte de recherche (obligatoire).
    pub fts_text: Option<String>,
    /// Filtre additionnel (`filter` Python ajoute au texte).
    pub filter: Option<String>,
    /// Type de metadonnee vise.
    pub metadata_type: Option<String>,
}

/// Nombre maximal de pairs interroges (`max_peers` du select distant).
const SEARCH_MAX_PEERS: usize = 20;

pub async fn remote_search(
    State(state): State<AppState>,
    Query(q): Query<RemoteSearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut query = q
        .fts_text
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("fts_text parameter missing"))?;
    if let Some(f) = &q.filter {
        query.push(' ');
        query.push_str(f);
    }
    let stack = state.session.ipv8().ok_or_else(|| {
        ApiError::bad_request("ipv8 stack inactive (recherche distante impossible)")
    })?;

    // `send_search_request` Python : select vers les pairs connus de
    // la community. Les reponses arrivent en pushback et sont
    // integrees a `channel_node` par le provider.
    let peers = stack
        .network
        .peers_for_service(&tribler_ipv8::content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID);
    let mut json = serde_json::json!({
        "txt_filter": query,
        "metadata_type": [300, 400],
    });
    if let Some(mt) = &q.metadata_type {
        json["metadata_type"] = serde_json::json!(mt);
    }
    let body = serde_json::to_vec(&json).map_err(|e| ApiError::internal(e.to_string()))?;

    let uuid = uuid::Uuid::new_v4().to_string();
    let mut queried = Vec::new();
    for peer in peers.iter().take(SEARCH_MAX_PEERS) {
        if let Some(addr) = &peer.address {
            // `notify_gui` Python (`send_search_request`) : chaque
            // reponse du pair notifie `remote_query_results` avec le
            // kwargs d'origine, l'uuid de la requete et le mid du pair.
            let notifier = state.session.notifier().clone();
            let (uuid_c, query_c, mid) = (uuid.clone(), query.clone(), hex::encode(peer.mid));
            let cb: tribler_ipv8::content_discovery::SelectCallback =
                std::sync::Arc::new(move |results| {
                    notifier.notify(tribler_core::Notification::RemoteQueryResults {
                        query: query_c.clone(),
                        results,
                        uuid: uuid_c.clone(),
                        peer: mid.clone(),
                    });
                });
            // `content_discovery_community/enabled=false` : pas de
            // community -> pas de select (la recherche locale REST
            // reste disponible via les autres endpoints).
            if let Some(cd) = &stack.content_discovery {
                if cd
                    .send_remote_select_cb(addr, body.clone(), cb)
                    .await
                    .is_ok()
                {
                    queried.push(hex::encode(peer.mid));
                }
            }
        }
    }
    Ok(Json(serde_json::json!({
        "request_uuid": uuid,
        "peers": queried,
    })))
}
