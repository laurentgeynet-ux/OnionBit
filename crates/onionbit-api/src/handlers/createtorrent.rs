//! Handlers `/api/createtorrent` — equivalent de
//! `tribler.core.libtorrent.restapi.create_torrent_endpoint`.
//!
//! La creation de `.torrent` est deleguee a `librqbit::create_torrent`
//! (BEP-52/`v1` pieces SHA-1 — meme pipeline que rqbit).

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Corps de `POST /api/createtorrent` (`CreateTorrentRequest` Python :
/// `files`, `name`, `description`, `tracker`, `export_dir`,
/// `piece_length`).
#[derive(Debug, Deserialize)]
pub struct CreateTorrentRequest {
    /// Fichiers/repertoires sources (chemins locaux du daemon).
    pub files: Vec<String>,
    /// Nom du torrent (defaut : nom du premier fichier).
    pub name: Option<String>,
    /// Description (commentaire du `.torrent`).
    pub description: Option<String>,
    /// URL de tracker (`announce`).
    pub tracker: Option<String>,
    /// Repertoire d'export du `.torrent` produit.
    pub export_dir: Option<String>,
    /// Taille de piece (`None` = choix automatique rqbit).
    pub piece_length: Option<u32>,
}

/// `POST /api/createtorrent` — cree un `.torrent` depuis des fichiers
/// locaux. Plusieurs chemins sont emballes dans un torrent
/// multi-fichier (Python : un torrent par fichier — nous fusionnons).
pub async fn create_torrent(
    State(_state): State<AppState>,
    Json(req): Json<CreateTorrentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.files.is_empty() {
        return Err(ApiError::bad_request("files parameter is empty"));
    }
    // Librqbit cree un torrent par chemin racine ; pour rester fidele
    // a l'endpoint (qui cree un torrent par fichier), on cree pour
    // chaque chemin et on rend la liste des resultats.
    let mut results = Vec::new();
    for file in &req.files {
        let path = std::path::Path::new(file);
        if !path.exists() {
            return Err(ApiError::bad_request(format!(
                "path does not exist: {file}"
            )));
        }
        let opts = librqbit::CreateTorrentOptions {
            name: req.name.as_deref(),
            trackers: req.tracker.iter().cloned().collect(),
            piece_length: req.piece_length,
        };
        // Un thread blocking suffit pour le hachage des pieces.
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
        let result = librqbit::create_torrent(path, opts, &spawner)
            .await
            .map_err(|e| ApiError::internal(format!("error creating torrent: {e}")))?;
        let bytes = result
            .as_bytes()
            .map_err(|e| ApiError::internal(format!("serialization error: {e}")))?;
        let infohash = onionbit_crypto::hash::to_hex(&result.info_hash().0);

        // Export si `export_dir` fourni (ou sous le nom du torrent dans
        // le repertoire parent du fichier source, comme Python).
        let mut written = None;
        let export = req
            .export_dir
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| path.parent().map(|p| p.to_path_buf()));
        if let Some(dir) = export {
            let name = req
                .name
                .clone()
                .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| infohash.clone());
            let dest = dir.join(format!("{name}.torrent"));
            std::fs::write(&dest, &bytes)
                .map_err(|e| ApiError::internal(format!("writing torrent file: {e}")))?;
            written = Some(dest.display().to_string());
        }
        results.push(serde_json::json!({
            "torrent": hex::encode(&bytes),
            "infohash": infohash,
            "path": written,
        }));
    }
    Ok(Json(serde_json::json!({ "results": results })))
}

/// Corps de `POST /api/createtorrent/dryrun`.
#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    /// Repertoire a tester en ecriture.
    pub export_dir: Option<String>,
}

/// `POST /api/createtorrent/dryrun` — verifie qu'un repertoire est
/// accessible en ecriture (`probe_writable` Python).
pub async fn dry_run(Json(req): Json<DryRunRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = req
        .export_dir
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| ApiError::bad_request("export_dir parameter missing"))?;
    if !dir.is_dir() {
        return Err(ApiError::bad_request(format!(
            "repertoire inexistant: {}",
            dir.display()
        )));
    }
    let probe = dir.join(".onionbit-createtorrent-probe");
    std::fs::write(&probe, b"")
        .and_then(|_| std::fs::remove_file(&probe))
        .map_err(|e| {
            ApiError::bad_request(format!("{} non accessible en ecriture: {e}", dir.display()))
        })?;
    Ok(Json(serde_json::json!({ "writable": true })))
}
