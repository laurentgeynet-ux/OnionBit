//! Handlers `/api/downloads` — equivalent de
//! `tribler.core.libtorrent.restapi.downloads_endpoint`.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::dto::DownloadInfo;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/downloads` — liste tous les telechargements.
///
/// Les parametres `get_peers`/`get_pieces`/`get_availability`/`infohash`
/// `excluded` de l'API Python sont acceptes mais seul `infohash` (filtre)
/// est implemente a ce stade (cf. mapping doc).
pub async fn get_downloads(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<DownloadQuery>,
) -> Json<serde_json::Value> {
    let downloads: Vec<DownloadInfo> = state
        .session
        .downloads()
        .iter()
        .filter(|s| {
            params
                .infohash
                .as_deref()
                .map(|ih| ih.eq_ignore_ascii_case(&s.info_hash))
                .unwrap_or(true)
        })
        .filter(|s| {
            params
                .excluded
                .as_deref()
                .map(|ih| !ih.eq_ignore_ascii_case(&s.info_hash))
                .unwrap_or(true)
        })
        .map(DownloadInfo::from_stats)
        .collect();
    Json(serde_json::json!({
        "downloads": downloads,
        // `checkpoints`/`clierrors` : champs Python, emis pour compat.
        "checkpoints": { "total": downloads.len(), "loaded": downloads.len(), "all_loaded": true },
        "clierrors": 0,
    }))
}

/// Query params acceptes par `GET /api/downloads`.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadQuery {
    /// Filtre sur un info-hash precis.
    pub infohash: Option<String>,
    /// Exclut un info-hash.
    pub excluded: Option<String>,
    /// Compat (non implemente).
    pub get_peers: Option<String>,
    /// Compat (non implemente).
    pub get_pieces: Option<String>,
    /// Compat (non implemente).
    pub get_availability: Option<String>,
}

/// Corps de `PUT /api/downloads`.
#[derive(Debug, Deserialize)]
pub struct AddDownloadRequest {
    /// Magnet ou URI http(s) pointant un `.torrent`.
    pub uri: Option<String>,
    /// Chemin local d'un fichier `.torrent` sur le disque du daemon.
    pub torrent: Option<String>,
    /// Repertoire de destination (defaut : config du daemon).
    pub destination: Option<String>,
    /// Nombre de sauts anonymes (ignore tant que les tunnels ne sont
    /// pas implementes — etape 12).
    pub anon_hops: Option<u32>,
    /// Demarrer en pause.
    pub paused: Option<bool>,
}

/// `PUT /api/downloads` — ajoute un telechargement.
pub async fn add_download(
    State(state): State<AppState>,
    Json(req): Json<AddDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.anon_hops.unwrap_or(0) > 0 {
        return Err(ApiError::bad_request(
            "anon_hops non supporte tant que les tunnels ne sont pas implementes (etape 12)",
        ));
    }
    let paused = req.paused.unwrap_or(false);
    let dl = if let Some(uri) = &req.uri {
        state.session.add_download(uri, paused).await?
    } else if let Some(path) = &req.torrent {
        let bytes = std::fs::read(path)
            .map_err(|e| ApiError::bad_request(format!("lecture du .torrent: {e}")))?;
        state.session.add_torrent_bytes(bytes, paused).await?
    } else {
        return Err(ApiError::bad_request("missing uri or torrent"));
    };
    Ok(Json(serde_json::json!({
        "started": true,
        "infohash": dl.info_hash_hex(),
        "name": dl.name().unwrap_or_default(),
    })))
}

/// Corps de `DELETE /api/downloads/{infohash}`.
#[derive(Debug, Deserialize)]
pub struct RemoveDownloadRequest {
    /// Supprimer aussi les donnees sur disque.
    pub remove_data: Option<bool>,
}

/// `DELETE /api/downloads/{infohash}` — supprime un telechargement.
pub async fn delete_download(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<RemoveDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .session
        .remove(&infohash, req.remove_data.unwrap_or(false))
        .await?;
    Ok(Json(serde_json::json!({
        "removed": true,
        "infohash": infohash,
    })))
}

/// Corps de `PATCH /api/downloads/{infohash}`.
#[derive(Debug, Deserialize)]
pub struct UpdateDownloadRequest {
    /// `"resume"`, `"stop"`, `"recheck"` (recheck non supporte), ou
    /// `"move_storage"` (non supporte).
    pub state: Option<String>,
    /// anon_hops : reserve, refuse si combine (comme en Python).
    pub anon_hops: Option<u32>,
}

/// `PATCH /api/downloads/{infohash}` — modifie l'etat.
pub async fn update_download(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<UpdateDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.anon_hops.is_some() {
        return Err(ApiError::bad_request(
            "anon_hops non supporte tant que les tunnels ne sont pas implementes (etape 12)",
        ));
    }
    let modified = match req.state.as_deref() {
        Some("resume") => {
            state.session.resume(&infohash).await?;
            true
        }
        Some("stop") => {
            state.session.pause(&infohash).await?;
            true
        }
        Some("recheck") => {
            return Err(ApiError::bad_request("recheck non implemente"));
        }
        Some("move_storage") => {
            return Err(ApiError::bad_request("move_storage non implemente"));
        }
        Some(_) => {
            return Err(ApiError::bad_request("unknown state parameter"));
        }
        None => false,
    };
    Ok(Json(serde_json::json!({
        "modified": modified,
        "infohash": infohash,
    })))
}
