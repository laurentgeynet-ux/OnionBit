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

/// Recherche le download courant — lookup strict par info-hash hex
/// (`unhexlify` Python ; un hash tout-chiffres comme `"00..0"` n'est
/// jamais interprete comme un id interne).
fn find(state: &AppState, infohash: &str) -> Result<tribler_bittorrent::Download, ApiError> {
    state
        .session
        .find_download_hex(infohash)
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

/// Corps optionnel de `POST /api/downloads/{ih}/clone_public`.
#[derive(Debug, Default, Deserialize)]
pub struct ClonePublicRequest {
    /// Sauts anonymes du jumeau public (defaut :
    /// `download_defaults/number_hops`, minimum 1 — comme le
    /// `hops=-1` Python qui prend le defaut configure).
    pub anon_hops: Option<u32>,
}

/// `POST /api/downloads/{ih}/clone_public` — extension (pas
/// d'equivalent Python) : cree un jumeau public du download
/// (flag `private` et trackers retires -> nouvel info-hash,
/// DHT/PEX reactives) et l'ajoute en seed anonyme sur les memes
/// fichiers. Sert a repartager un contenu de tracker prive vers
/// l'essaim anonyme sans re-telechargement.
pub async fn clone_public(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    body: Option<Json<ClonePublicRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    find(&state, &infohash)?;
    let hops = body.and_then(|b| b.0.anon_hops);
    let dl = state
        .session
        .clone_public(&infohash, hops)
        .await
        .map_err(|e| ApiError::internal_handled(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "started": true,
        "infohash": dl.info_hash_hex(),
        "name": dl.name().unwrap_or_default(),
    })))
}

/// Corps des requetes tracker (`url` obligatoire — message Python
/// `"url parameter missing"` sur 400).
#[derive(Debug, Deserialize)]
pub struct TrackerRequest {
    /// URL du tracker.
    pub url: Option<String>,
}

/// `PUT /api/downloads/{ih}/trackers` — ajoute un tracker au torrent.
/// Persiste dans `extra_trackers` (rejoue au re-add ; rqbit ne
/// reannonce pas un tracker ajoute a chaud — ecart documente).
pub async fn add_tracker(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TrackerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 404 avant la validation du corps, comme `add_tracker` Python.
    find(&state, &infohash)?;
    let url = req
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::bad_request("url parameter missing"))?;
    state
        .session
        .add_tracker(&infohash, &url)
        .await
        .map_err(|e| ApiError::internal_handled(e.to_string()))?;
    Ok(Json(serde_json::json!({ "added": true })))
}

/// `PUT /api/downloads/{ih}/default_trackers` — ajoute les trackers
/// de `download_defaults/trackers_file` au torrent.
pub async fn add_default_trackers(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    find(&state, &infohash)?;
    state
        .session
        .add_default_trackers(&infohash)
        .await
        .map_err(|e| ApiError::internal_handled(e.to_string()))?;
    Ok(Json(serde_json::json!({ "added": true })))
}

/// `DELETE /api/downloads/{ih}/trackers` — retire un tracker
/// (persiste ; effectif au prochain re-add, rqbit n'expose pas
/// `replace_trackers` a chaud — ecart documente).
pub async fn remove_tracker(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TrackerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    find(&state, &infohash)?;
    let url = req
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::bad_request("url parameter missing"))?;
    state
        .session
        .remove_tracker(&infohash, &url)
        .await
        .map_err(|e| ApiError::internal_handled(e.to_string()))?;
    Ok(Json(serde_json::json!({ "removed": true })))
}

/// `PUT /api/downloads/{ih}/tracker_force_announce` — force une
/// re-annonce. `{"forced": true}` est rendu meme si l'URL ne correspond
/// a aucun tracker (comportement Python : la boucle ne trouve rien et
/// repond quand meme `forced: true`).
pub async fn tracker_force_announce(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TrackerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    find(&state, &infohash)?;
    let _url = req
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::bad_request("url parameter missing"))?;
    state
        .session
        .force_announce(&infohash)
        .await
        .map_err(|e| ApiError::internal_handled(e.to_string()))?;
    Ok(Json(serde_json::json!({ "forced": true })))
}

/// `TrackerStatusDict` Python : `{url, peers, seeds, leeches,
/// status}` — valeurs par defaut de `get_tracker_status` tant que le
/// tracker n'a pas ete scrape (`peers=-1`, `"Not contacted yet"`).
fn tracker_status_json(url: String) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "peers": -1,
        "seeds": -1,
        "leeches": -1,
        "status": "Not contacted yet",
    })
}

/// Liste `tracker_info` complete : trackers reels + pseudo-entrees
/// `[DHT]`/`[PeX]` ajoutees par `get_tracker_status` Python.
/// `dht_running` pilote le statut `[DHT]` (`Working`/`Disabled`) ;
/// librqbit n'expose pas les compteurs de pairs par source (`0`).
pub(crate) fn trackers_json(urls: Vec<String>, dht_running: bool) -> Vec<serde_json::Value> {
    let mut out: Vec<_> = urls.into_iter().map(tracker_status_json).collect();
    out.push(serde_json::json!({
        "url": "[DHT]",
        "peers": 0,
        "seeds": -1,
        "leeches": -1,
        "status": if dht_running { "Working" } else { "Disabled" },
    }));
    out.push(serde_json::json!({
        "url": "[PeX]",
        "peers": 0,
        "seeds": -1,
        "leeches": -1,
        "status": "Working",
    }));
    out
}

/// `GET /api/downloads/{ih}/trackers` — trackers du torrent
/// (`announce`/`announce-list` + ajouts a chaud). Extension Rust :
/// Python n'expose pas ce GET ; le shape suit `tracker_info`.
pub async fn get_trackers(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = find(&state, &infohash)?;
    let dht = state.session.engine().config().enable_dht;
    Ok(Json(
        serde_json::json!({ "tracker_info": trackers_json(dl.trackers(), dht) }),
    ))
}

/// `GET /api/downloads/{ih}/files` — fichiers du torrent avec
/// progression individuelle (`DownloadFile` -> JSON).
pub async fn get_download_files(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = find(&state, &infohash)?;
    let selected = dl.only_files();
    let files: Vec<_> = dl
        .files()
        .ok_or_else(|| ApiError::not_found("metainfo non disponible (magnet non resolu)"))?
        .iter()
        .map(|f| {
            serde_json::json!({
                "index": f.index,
                "name": f.name,
                "size": f.length,
                // Python : `file_index in selected_files` (tous si
                // aucune selection).
                "included": selected
                    .as_ref()
                    .is_none_or(|sel| sel.contains(&f.index)),
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
