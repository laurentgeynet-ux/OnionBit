//! Options par telechargement a l'ajout — traduction Rust du
//! `DownloadConfig` checkpointe par Tribler (`dlcheckpoints/`).
//!
//! Librqbit n'expose ces reglages qu'a l'ajout du torrent : la couche
//! domaine (`tribler-core`) les persiste en base et les reapplique a
//! chaque (re)creation du telechargement (restauration, `update_hops`,
//! `recheck`, `move_storage`).

use std::path::PathBuf;

/// Reglages par telechargement appliques a l'ajout.
#[derive(Debug, Clone, Default)]
pub struct AddDownloadOptions {
    /// Demarrer en pause (`user_stopped` persiste).
    pub paused: bool,
    /// Dossier de sortie explicite (`None` = dossier de session).
    pub output_folder: Option<PathBuf>,
    /// Indices des fichiers a telecharger (`None` = tous ;
    /// `Some(vec![])` = aucun — comme `selected_files` Python).
    pub only_files: Option<Vec<usize>>,
    /// Trackers additionnels (`extra_trackers` persistes — rqbit les
    /// fusionne a `announce`/`announce-list` a l'ajout).
    pub trackers: Vec<String>,
    /// Limite d'upload par torrent en octets/s (`None` = illimite).
    /// Appliquee via `initial ratelimits` rqbit a l'ajout — pas de
    /// mutation a chaud exposee par librqbit 9.x (ecart documente).
    pub upload_limit_bps: Option<u64>,
    /// Limite de download par torrent en octets/s.
    pub download_limit_bps: Option<u64>,
}
