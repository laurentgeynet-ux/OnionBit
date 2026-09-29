//! DTOs JSON de l'API REST — champs nommes comme les reponses Python
//! de `downloads_endpoint.py` (cf. `docs/reference_tribler/
//! api_rest_mapping.md` pour les ecarts).

use tribler_bittorrent::{DownloadState, DownloadStats};

/// Statut d'un telechargement tel que l'API Python l'expose
/// (`download_state.DownloadStatus`).
///
/// Valeurs numeriques identiques a l'enum Python.
#[derive(Debug, Clone, Copy)]
pub struct StatusCode(pub i32);

/// Mappe l'etat domaine vers le statut Python `DownloadStatus`.
///
/// Correspondance (`download_state.DownloadStatus`) :
/// `ALLOCATING_DISKSPACE=0, WAITING_FOR_HASHCHECK=1, HASHCHECKING=2,
/// DOWNLOADING=3, SEEDING=4, STOPPED=5, STOPPED_ON_ERROR=6,
/// METADATA=7, LOADING=8, EXIT_NODES=9, MOVING=10, QUEUED=11`.
fn status_of(state: DownloadState, stats: &DownloadStats) -> (StatusCode, &'static str) {
    match state {
        DownloadState::Initializing => {
            if stats.total_bytes == 0 {
                (StatusCode(7), "METADATA")
            } else {
                (StatusCode(1), "WAITING_FOR_HASHCHECK")
            }
        }
        DownloadState::Checking => (StatusCode(2), "HASHCHECKING"),
        DownloadState::Downloading => (StatusCode(3), "DOWNLOADING"),
        DownloadState::Seeding => (StatusCode(4), "SEEDING"),
        DownloadState::Paused | DownloadState::Stopped => (StatusCode(5), "STOPPED"),
        DownloadState::Error => (StatusCode(6), "STOPPED_ON_ERROR"),
    }
}

/// Entree de `GET /api/downloads` — miroir du dict `info` Python.
///
/// Les champs que librqbit ne fournit pas encore sont emis avec leurs
/// valeurs par defaut Python (`0`, `""`, `[]`) pour preserver la
/// compatibilite des clients.
#[derive(Debug, serde::Serialize)]
pub struct DownloadInfo {
    /// Nom du contenu.
    pub name: String,
    /// Fraction de progression `[0.0, 1.0]`.
    pub progress: f64,
    /// Info-hash hex.
    pub infohash: String,
    /// Debit descendant (octets/s).
    pub speed_down: u64,
    /// Debit montant (octets/s).
    pub speed_up: u64,
    /// Nom du statut (`DOWNLOADING`, `SEEDING`, …).
    pub status: &'static str,
    /// Code numerique du statut (`DownloadStatus` Python).
    pub status_code: i32,
    /// Taille totale.
    pub size: u64,
    /// ETA en secondes (float, comme `download_state.get_eta()`).
    pub eta: f64,
    /// Pairs connectes (leechers + seeds confondus cote librqbit).
    pub num_peers: u32,
    /// Seeds connus (0 si trackers non scrapes).
    pub num_seeds: u32,
    /// Pairs connectes actifs.
    pub num_connected_peers: u32,
    /// Seeds connectes (non distingues par librqbit).
    pub num_connected_seeds: u32,
    /// Upload cumule.
    pub all_time_upload: u64,
    /// Download cumule.
    pub all_time_download: u64,
    /// Ratio all-time upload/download.
    pub all_time_ratio: f64,
    /// Liste des trackers (non implemente — toujours `[]`).
    pub trackers: Vec<serde_json::Value>,
    /// Nombre de sauts anonymes (0 = non anonyme, tunnels a l'etape 12).
    pub hops: u32,
    /// Telechargement anonyme actif.
    pub anon_download: bool,
    /// Seeding sur (a implementer avec les tunnels).
    pub safe_seeding: bool,
    /// Limite d'upload octets/s (0 = illimite).
    pub upload_limit: u64,
    /// Limite de download octets/s.
    pub download_limit: u64,
    /// Ratio de seed vise.
    pub seeding_ratio: f64,
    /// Repertoire de destination.
    pub destination: String,
    /// Repertoire de fichiers termines.
    pub completed_dir: String,
    /// Nombre total de pieces.
    pub total_pieces: usize,
    /// Erreur (`""` si aucune — format Python).
    pub error: String,
    /// Timestamp d'ajout (secondes Unix).
    pub time_added: i64,
    /// Timestamp de fin.
    pub time_finished: i64,
    /// Position dans la file (-1 : non gere).
    pub queue_position: i64,
    /// Auto-managed (non gere par librqbit — toujours false).
    pub auto_managed: bool,
    /// Arrete par l'utilisateur.
    pub user_stopped: bool,
    /// Flux multimedia diffusable.
    pub streamable: bool,
    /// Extension (pas Python) : flag `private` du metainfo —
    /// permet au client de proposer « republier en anonyme ».
    pub private: bool,
}

impl DownloadInfo {
    /// Traduit un instantane domaine en DTO.
    pub fn from_stats(s: &DownloadStats) -> Self {
        let (code, name) = status_of(s.state, s);
        let video_exts = ["mp4", "m4v", "mov", "mkv"];
        // Approximation tant que les noms de fichiers ne sont pas
        // exposes dans DownloadStats : on teste l'extension du nom du
        // torrent (correct pour les torrents mono-fichier).
        let streamable = s
            .name
            .as_deref()
            .map(|n| video_exts.iter().any(|e| n.ends_with(e)))
            .unwrap_or(false);
        // Fidele a `DownloadState.get_eta()` Python :
        // `(1 - progress) * total_size / max(download_rate, 1e-6)`
        // (0.0 sans taille connue ; progress=1 -> 0).
        let eta = if s.total_bytes == 0 {
            0.0
        } else {
            (1.0 - s.progress()) * s.total_bytes as f64 / (s.download_speed as f64).max(0.000001)
        };
        Self {
            name: s.name.clone().unwrap_or_default(),
            progress: s.progress(),
            infohash: s.info_hash.clone(),
            speed_down: s.download_speed,
            speed_up: s.upload_speed,
            status: name,
            status_code: code.0,
            size: s.total_bytes,
            eta,
            num_peers: s.peers_live,
            num_seeds: 0,
            num_connected_peers: s.peers_live,
            num_connected_seeds: 0,
            all_time_upload: s.uploaded_bytes,
            all_time_download: s.progress_bytes,
            all_time_ratio: if s.progress_bytes == 0 {
                0.0
            } else {
                s.uploaded_bytes as f64 / s.progress_bytes as f64
            },
            trackers: Vec::new(),
            hops: 0,
            anon_download: false,
            safe_seeding: false,
            upload_limit: 0,
            download_limit: 0,
            seeding_ratio: 0.0,
            destination: String::new(),
            completed_dir: String::new(),
            total_pieces: s.file_progress.len(),
            error: s.error.clone().unwrap_or_default(),
            time_added: 0,
            time_finished: 0,
            queue_position: -1,
            auto_managed: false,
            user_stopped: matches!(s.state, DownloadState::Paused),
            streamable,
            private: false,
        }
    }
}
