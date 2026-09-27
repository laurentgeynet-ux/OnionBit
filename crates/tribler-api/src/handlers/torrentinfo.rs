//! Handlers `/api/torrentinfo` — equivalent de
//! `tribler.core.libtorrent.restapi.torrentinfo_endpoint`.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Construit la reponse `{"files", "name", ...}` d'un `TorrentMeta`
/// (`get_torrent_info` Python).
fn metainfo_json(
    meta: &tribler_format::torrent::TorrentMeta,
    download_exists: bool,
    valid_cert: bool,
) -> serde_json::Value {
    let multi = meta.files.len() > 1;
    let files: Vec<_> = meta
        .files
        .iter()
        .enumerate()
        .map(|(index, f)| {
            let name = if multi {
                f.path.join("/")
            } else {
                f.path.last().cloned().unwrap_or_else(|| meta.name.clone())
            };
            serde_json::json!({
                "index": index,
                "name": name,
                "size": f.length,
            })
        })
        .collect();
    serde_json::json!({
        "files": files,
        "name": meta.name,
        "description": meta.comment.clone().unwrap_or_default(),
        "download_exists": download_exists,
        "valid_certificate": valid_cert,
    })
}

/// Corps de `POST /api/torrentinfo/uri`.
#[derive(Debug, Deserialize)]
pub struct TorrentInfoUriRequest {
    /// `file://`, `http(s)://` ou `magnet:` URI.
    pub uri: Option<String>,
    /// Sauts anonymes (metainfo via tunnel — ignore pour l'instant,
    /// reseau direct requis).
    pub hops: Option<u32>,
    /// Ne pas resoudre la metainfo d'un magnet (`skipmagnet`).
    pub skipmagnet: Option<bool>,
}

/// `POST /api/torrentinfo/uri` — metainfo depuis une URI.
///
/// - `file:`/`http(s):` → `.torrent` parse (fetch anti-SSRF via
///   `services::fetch_checked` pour http).
/// - `magnet:` → resolution via la DHT du moteur (`skipmagnet` rend
///   une reponse immediate sans metainfo, comme Python).
pub async fn get_torrent_info(
    State(state): State<AppState>,
    Json(req): Json<TorrentInfoUriRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let uri = req
        .uri
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::bad_request("uri parameter missing"))?;
    if req.hops.unwrap_or(0) > 0 && state.session.ipv8().is_none() {
        return Err(ApiError::bad_request("hops > 0 mais anonymat inactif"));
    }
    let skipmagnet = req.skipmagnet.unwrap_or(false);

    let meta = if let Some(path) = uri.strip_prefix("file://") {
        let bytes = std::fs::read(path)
            .map_err(|e| ApiError::bad_request(format!("lecture du .torrent: {e}")))?;
        tribler_format::torrent::TorrentMeta::parse(&bytes)
            .map_err(|e| ApiError::internal(format!("error while decoding torrent file: {e}")))?
    } else if uri.starts_with("http://") || uri.starts_with("https://") {
        let resp = tribler_core::services::fetch_checked(&uri, &state.session.config().ip_policy)
            .await
            .map_err(|e| ApiError::internal(format!("Error while querying http uri: {e}")))?;
        let body = tribler_core::services::read_body_limited(resp)
            .await
            .map_err(|e| ApiError::internal(format!("Error while reading response: {e}")))?;
        if body.starts_with(b"magnet") {
            return Err(ApiError::bad_request(
                "uri pointe vers un magnet — utiliser magnet:?xt=... directement",
            ));
        }
        tribler_format::torrent::TorrentMeta::parse(&body)
            .map_err(|e| ApiError::internal(format!("Could not read torrent from {uri}: {e}")))?
    } else if uri.starts_with("magnet:") {
        let magnet = tribler_format::magnet::MagnetLink::parse(&uri)
            .map_err(|e| ApiError::bad_request(format!("Error while getting an infohash: {e}")))?;
        if skipmagnet {
            return Ok(Json(serde_json::json!({
                "metainfo": "",
                "download_exists": false,
                "valid_certificate": true,
            })));
        }
        // Resolution des metadonnees par le moteur (magnet -> DHT).
        let dl = state
            .session
            .add_download_anon(&uri, true, 0)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        dl.wait_initialized()
            .await
            .map_err(|e| ApiError::internal(format!("metainfo error: {e}")))?;
        let bytes = dl
            .torrent_bytes()
            .ok_or_else(|| ApiError::internal("metainfo error"))?;
        let meta = tribler_format::torrent::TorrentMeta::parse(&bytes)
            .map_err(|e| ApiError::internal(format!("metainfo error: {e}")))?;
        let _ = state.session.remove(&dl.info_hash_hex(), false).await;
        // Ne rien notifier : c'etait une sonde de metainfo.
        let exists = state
            .session
            .find_download(&magnet.info_hash_hex())
            .is_some();
        return Ok(Json(metainfo_json(&meta, exists, true)));
    } else {
        return Err(ApiError::bad_request("invalid uri"));
    };

    let exists = state.session.find_download(&meta.info_hash_hex()).is_some();
    Ok(Json(metainfo_json(&meta, exists, true)))
}

/// `PUT /api/torrentinfo/file` — metainfo depuis un `.torrent` envoye
/// en corps brut (bencode) ou `{"torrent": "<hex>"}`.
pub async fn get_torrent_info_from_file(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Accepte le corps brut ou un JSON {"torrent": "<hex>"} (le GUI
    // Python poste le fichier encode hex).
    let bytes = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(v) => v
            .get("torrent")
            .and_then(|t| t.as_str())
            .and_then(|h| hex::decode(h).ok())
            .unwrap_or_else(|| body.to_vec()),
        Err(_) => body.to_vec(),
    };
    let meta = tribler_format::torrent::TorrentMeta::parse(&bytes)
        .map_err(|e| ApiError::internal(format!("error while decoding torrent file: {e}")))?;
    let exists = state.session.find_download(&meta.info_hash_hex()).is_some();
    Ok(Json(metainfo_json(&meta, exists, true)))
}
