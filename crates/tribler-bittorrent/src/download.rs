//! Vue d'un telechargement individuel (`Download` + `DownloadStats`).
//!
//! Les types de ce fichier sont la representation **domaine** d'un
//! telechargement, decouplee de `librqbit` : l'API REST et le domaine
//! `tribler-core` ne doivent manipuler que ces types.

use std::sync::Arc;

use librqbit::TorrentStatsState;

/// Etat de haut niveau d'un telechargement, aligne sur les etats que
/// l'API REST Tribler historique expose (`DLSTATUS_*` de
/// `tribler.core.libtorrent.download_manager.download_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Initialisation (lecture du metainfo, allocation fichiers).
    Initializing,
    /// Verification des pieces deja presentes (fastresume ou recheck).
    Checking,
    /// Telechargement actif.
    Downloading,
    /// Telechargement termine, en seed.
    Seeding,
    /// En pause.
    Paused,
    /// En erreur (detail dans `DownloadStats::error`).
    Error,
    /// Arrete / supprime de la session.
    Stopped,
}

/// Instantane d'avancement d'un telechargement.
#[derive(Debug, Clone)]
pub struct DownloadStats {
    /// Identifiant numerique interne (attribution `librqbit`).
    pub id: usize,
    /// Info-hash v1 en hexadecimal minuscule.
    pub info_hash: String,
    /// Nom du contenu.
    pub name: Option<String>,
    /// Etat courant.
    pub state: DownloadState,
    /// Octets utiles deja verifies/ecrits.
    pub progress_bytes: u64,
    /// Taille totale du contenu.
    pub total_bytes: u64,
    /// Octets envoyes aux pairs.
    pub uploaded_bytes: u64,
    /// Progression par fichier (octets).
    pub file_progress: Vec<u64>,
    /// Telechargement termine (toutes les pieces presentes).
    pub finished: bool,
    /// Torrent actif (connexions etablies).
    pub live: bool,
    /// Message d'erreur si `state == Error`.
    pub error: Option<String>,
}

impl DownloadStats {
    /// Fraction de progression `[0.0, 1.0]`.
    pub fn progress(&self) -> f64 {
        if self.total_bytes == 0 {
            return f64::from(self.finished as u8);
        }
        (self.progress_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }
}

/// Handle d'un telechargement gere par la session.
///
/// Clone leger (`Arc` interne) : peut etre partage entre les couches.
#[derive(Clone)]
pub struct Download {
    pub(crate) inner: Arc<librqbit::ManagedTorrent>,
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download")
            .field("id", &self.id())
            .field("info_hash", &self.info_hash_hex())
            .finish()
    }
}

impl Download {
    /// Identifiant numerique interne.
    pub fn id(&self) -> usize {
        self.inner.id()
    }

    /// Info-hash v1 (20 octets).
    pub fn info_hash(&self) -> tribler_crypto::hash::InfoHashV1 {
        self.inner.info_hash().0
    }

    /// Info-hash v1 en hexadecimal minuscule.
    pub fn info_hash_hex(&self) -> String {
        tribler_crypto::hash::to_hex(&self.info_hash())
    }

    /// Nom du torrent si les metadonnees sont deja connues
    /// (`None` tant qu'un magnet n'a pas recu ses metadonnees).
    pub fn name(&self) -> Option<String> {
        self.inner.name()
    }

    /// Repertoire effectif d'ecriture des fichiers.
    pub fn output_folder(&self) -> std::path::PathBuf {
        self.inner.output_folder().to_path_buf()
    }

    /// Le torrent est-il en pause ?
    pub fn is_paused(&self) -> bool {
        self.inner.is_paused()
    }

    /// Instantane de l'etat courant.
    pub fn stats(&self) -> DownloadStats {
        let s = self.inner.stats();
        let state = match s.state {
            TorrentStatsState::Error => DownloadState::Error,
            TorrentStatsState::Paused => DownloadState::Paused,
            TorrentStatsState::Initializing { paused } => {
                if paused {
                    DownloadState::Paused
                } else if s.progress_bytes > 0 {
                    DownloadState::Checking
                } else {
                    DownloadState::Initializing
                }
            }
            TorrentStatsState::Live => {
                if s.finished {
                    DownloadState::Seeding
                } else {
                    DownloadState::Downloading
                }
            }
        };
        DownloadStats {
            id: self.id(),
            info_hash: self.info_hash_hex(),
            name: self.name(),
            state,
            progress_bytes: s.progress_bytes,
            total_bytes: s.total_bytes,
            uploaded_bytes: s.uploaded_bytes,
            file_progress: s.file_progress,
            finished: s.finished,
            live: matches!(s.state, TorrentStatsState::Live),
            error: s.error,
        }
    }

    /// Attend que les metadonnees soient resolues (magnet -> info) et
    /// que le torrent soit initialise.
    pub async fn wait_initialized(&self) -> crate::Result<()> {
        self.inner
            .wait_until_initialized()
            .await
            .map_err(|e| crate::BtError::Engine(e.to_string()))
    }

    /// Attend la fin du telechargement (toutes les pieces).
    pub async fn wait_completed(&self) -> crate::Result<()> {
        self.inner
            .wait_until_completed()
            .await
            .map_err(|e| crate::BtError::Engine(e.to_string()))
    }
}
