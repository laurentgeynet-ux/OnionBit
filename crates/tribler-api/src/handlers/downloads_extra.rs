//! Sous-endpoints `/api/downloads/{infohash}` — fichiers, trackers,
//! `.torrent` et streaming (equivalent `downloads_endpoint` Python).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Recherche le download courant.
fn find(state: &AppState, infohash: &str) -> Result<tribler_bittorrent::Download, ApiError> {
    state
        .session
        .find_download(infohash)
        .ok_or_else(|| ApiError::not_found(format!("download {infohash} inconnu")))
}

/// `GET /api/downloads/{ih}/torrent` — `.torrent` brut (metainfo).
pub async fn get_download_torrent(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Response, ApiError> {
    let dl = find(&state, &infohash)?;
    let bytes = dl
        .torrent_bytes()
        .ok_or_else(|| ApiError::not_found("metainfo non disponible (magnet non resolu)"))?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-bittorrent")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{infohash}.torrent\""),
        )
        .body(Body::from(bytes))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `PUT /api/downloads/{ih}/trackers` — ajoute un tracker au torrent.
#[derive(Debug, Deserialize)]
pub struct AddTrackerRequest {
    /// URL du tracker.
    pub url: Option<String>,
}

pub async fn add_tracker(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<AddTrackerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = req
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::bad_request("url parameter missing"))?;
    let dl = find(&state, &infohash)?;
    dl.add_tracker(&url);
    Ok(Json(serde_json::json!({ "modified": true })))
}

/// `GET /api/downloads/{ih}/trackers` — trackers du torrent
/// (`announce`/`announce-list` + ajouts a chaud).
pub async fn get_trackers(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = find(&state, &infohash)?;
    let trackers: Vec<_> = dl
        .trackers()
        .into_iter()
        .map(|url| serde_json::json!({ "url": url }))
        .collect();
    Ok(Json(serde_json::json!({ "tracker_info": trackers })))
}

/// `GET /api/downloads/{ih}/files` — fichiers du torrent avec
/// progression individuelle (`DownloadFile` -> JSON).
pub async fn get_download_files(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = find(&state, &infohash)?;
    let files: Vec<_> = dl
        .files()
        .ok_or_else(|| ApiError::not_found("metainfo non disponible (magnet non resolu)"))?
        .iter()
        .map(|f| {
            serde_json::json!({
                "index": f.index,
                "name": f.name,
                "size": f.length,
                "included": true,
                "progress": f.progress,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "files": files })))
}

/// `GET /api/downloads/{ih}/stream/{fileindex}` — flux HTTP de
/// l'archive du fichier (chunks via la session ; utilise par le
/// lecteur media de Tribler).
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Resume a cet offset (`start` Python).
    pub start: Option<u64>,
    /// Taille de chunk demandee.
    pub chunksize: Option<u64>,
}

/// Taille de chunk par defaut du streaming (64 Kio, comme `stream.py`).
const DEFAULT_CHUNK: u64 = 65536;

pub async fn stream_file(
    State(state): State<AppState>,
    Path((infohash, fileindex)): Path<(String, usize)>,
    Query(q): Query<StreamQuery>,
) -> Result<Response, ApiError> {
    let dl = find(&state, &infohash)?;
    let start = q.start.unwrap_or(0);
    let _ = q.chunksize.unwrap_or(DEFAULT_CHUNK);
    let stream = dl
        .stream_file_from(fileindex, start)
        .await
        .map_err(|e| ApiError::bad_request(format!("fileindex {fileindex} invalide: {e}")))?;
    let length = stream.length;
    let rx = tokio_stream::wrappers::ReceiverStream::new(stream.rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, length.saturating_sub(start))
        .body(Body::from_stream(rx))
        .map_err(|e| ApiError::internal(e.to_string()))
}
