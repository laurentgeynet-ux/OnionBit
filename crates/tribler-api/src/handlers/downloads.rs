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
    let hops_map = state.session.anon_hops_map();
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
        .map(|s| {
            let mut info = DownloadInfo::from_stats(s);
            let hops = hops_map.get(&s.info_hash).copied().unwrap_or(0);
            info.hops = hops;
            info.anon_download = hops > 0;
            // Sante scrapee par le torrent checker (`num_seeds`/
            // `num_peers` = max(lt, scraped) en Python ; librqbit
            // n'expose pas les compteurs swarm lt — on retient le
            // scrape, coherent avec `metadata/torrents/.../health`).
            if let Some(ih) = tribler_crypto::hash::from_hex(&s.info_hash) {
                if let Ok(Some(ts)) = state
                    .session
                    .db()
                    .with(|c| tribler_db::health::get_torrent_state(c, &ih))
                {
                    info.num_seeds = ts.seeders.max(0) as u32;
                    info.num_peers = info.num_peers.max(ts.leechers.max(0) as u32);
                }
            }
            info
        })
        .collect();
    Json(serde_json::json!({
        "downloads": downloads,
        // `checkpoints` : champ Python emis pour compat. `clierrors`
        // = taille de la file d'erreurs CLI non lues (comme Python).
        "checkpoints": { "total": downloads.len(), "loaded": downloads.len(), "all_loaded": true },
        "clierrors": state.unhandled_cli.lock().unwrap().len(),
    }))
}

/// `GET /api/downloads/clierrors` — vide la file des erreurs CLI
/// (`get_unhandled_cli` Python : `{"errors": [...]}`, drain).
pub async fn get_cli_errors(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut log = state.unhandled_cli.lock().unwrap();
    let errors: Vec<String> = log.drain(..).collect();
    Json(serde_json::json!({ "errors": errors }))
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
    /// Nombre de sauts anonymes (0 = telechargement direct).
    /// Exige `safe_seeding` et la stack IPv8 avec anonymat actif.
    pub anon_hops: Option<u32>,
    /// Seeding anonyme — obligatoire si `anon_hops > 0` (Python).
    pub safe_seeding: Option<bool>,
    /// Demarrer en pause.
    pub paused: Option<bool>,
    /// Requete emise depuis un CLI : les erreurs sont journalisees
    /// dans la file `clierrors` (parametre `cli` Python).
    pub cli: Option<bool>,
}

/// Enregistre l'erreur dans la file CLI si `cli` est vrai, comme
/// `_add_err` Python, puis la retourne au client.
fn add_err(state: &AppState, msg: String, cli: bool) -> ApiError {
    if cli {
        state.push_cli_error(msg.clone());
    }
    ApiError::bad_request(msg)
}

/// `PUT /api/downloads` — ajoute un telechargement.
///
/// Fidele au Python : `anon_hops > 0` sans `safe_seeding` est refuse,
/// et le telechargement part sur la lane anonyme correspondante.
pub async fn add_download(
    State(state): State<AppState>,
    Json(req): Json<AddDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cli = req.cli.unwrap_or(false);
    let hops = req.anon_hops.unwrap_or(0);
    if hops > 0 && !req.safe_seeding.unwrap_or(false) {
        return Err(add_err(
            &state,
            "Cannot set anonymous download without safe seeding enabled".into(),
            cli,
        ));
    }
    let paused = req.paused.unwrap_or(false);
    let dl = if let Some(uri) = &req.uri {
        state
            .session
            .add_download_anon(uri, paused, hops)
            .await
            .map_err(|e| add_err(&state, e.to_string(), cli))?
    } else if let Some(path) = &req.torrent {
        let bytes = std::fs::read(path)
            .map_err(|e| add_err(&state, format!("lecture du .torrent: {e}"), cli))?;
        state
            .session
            .add_torrent_bytes_anon(bytes, paused, hops)
            .await
            .map_err(|e| add_err(&state, e.to_string(), cli))?
    } else {
        return Err(add_err(&state, "uri parameter missing".into(), cli));
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
    /// Nouveau nombre de sauts anonymes — doit etre le seul parametre
    /// de la requete (comme en Python).
    pub anon_hops: Option<u32>,
}

/// `PATCH /api/downloads/{infohash}` — modifie l'etat.
///
/// `anon_hops` seul : le telechargement est deplace sur le moteur a
/// `n` sauts (`update_hops` Python — suppression + re-creation).
pub async fn update_download(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<UpdateDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(hops) = req.anon_hops {
        if req.state.is_some() {
            return Err(ApiError::bad_request(
                "anon_hops must be the only parameter in this request",
            ));
        }
        if state.session.find_download(&infohash).is_none() {
            return Err(ApiError::not_found(format!(
                "this download does not exist: {infohash}"
            )));
        }
        state
            .session
            .update_hops(&infohash, hops)
            .await
            .map_err(invalid_state_as_bad_request)?;
        return Ok(Json(serde_json::json!({
            "modified": true,
            "infohash": infohash,
        })));
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

/// Les erreurs metier `InvalidState` des chemins anonymes (stack ipv8
/// inactive, lane indisponible) sont des erreurs de requete, pas des
/// 404 : les ressources introuvables sont testees explicitement avant.
fn invalid_state_as_bad_request(e: tribler_core::CoreError) -> ApiError {
    match e {
        tribler_core::CoreError::InvalidState(m) => ApiError::bad_request(m),
        other => ApiError::from(other),
    }
}
