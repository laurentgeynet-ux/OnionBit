// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Vue d'un telechargement individuel (`Download` + `DownloadStats`).
//!
//! Les types de ce fichier sont la representation **domaine** d'un
//! telechargement, decouplee de `librqbit` : l'API REST et le domaine
//! `onionbit-core` ne doivent manipuler que ces types.

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
    /// Octets reellement recus du reseau cette session
    /// (`fetched_bytes` rqbit — exclut les pieces verifiees par le
    /// controle de hash au demarrage, contrairement a
    /// `progress_bytes`).
    pub fetched_bytes: u64,
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
    /// Debit descendant en octets/s (0 si non actif).
    pub download_speed: u64,
    /// Debit montant en octets/s (0 si non actif).
    pub upload_speed: u64,
    /// Pairs connectes.
    pub peers_live: u32,
    /// Pairs en cours de connexion.
    pub peers_connecting: u32,
    /// Pairs en file d'attente.
    pub peers_queued: u32,
    /// Pairs vus (cumul).
    pub peers_seen: u32,
    /// Pairs morts/erreurs.
    pub peers_dead: u32,
    /// Temps restant estime, formate (ex. "3m 20s"). `None` si
    /// indeterminable.
    pub eta_human: Option<String>,
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
    /// Trackers ajoutes a chaud via l'API (`PUT /downloads/{ih}/trackers`).
    /// rqbit ne permet pas de muter `shared.trackers` apres
    /// l'initialisation ; on les fusionne dans `trackers()` et ils
    /// sont persistes dans la ligne `downloads` par le core.
    pub(crate) extra_trackers: Arc<std::sync::Mutex<Vec<String>>>,
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
    pub fn info_hash(&self) -> onionbit_crypto::hash::InfoHashV1 {
        self.inner.info_hash().0
    }

    /// Info-hash v1 en hexadecimal minuscule.
    pub fn info_hash_hex(&self) -> String {
        onionbit_crypto::hash::to_hex(&self.info_hash())
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

    /// Flag `private` du metainfo (DHT/PEX/LSD desactives —
    /// pairs uniquement via le tracker).
    pub fn is_private(&self) -> bool {
        self.inner
            .with_metadata(|m| m.info.info().private)
            .unwrap_or(false)
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
                } else if s.checking || s.progress_bytes > 0 {
                    // `checking` = `check()` rqbit reellement en cours
                    // (fastresume/recheck disque) — distinct de
                    // l'attente dans la file `concurrent_init_limit`,
                    // qui reste `Initializing` -> WAITING_FOR_HASHCHECK.
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
        let (
            download_speed,
            upload_speed,
            peers_live,
            peers_connecting,
            peers_queued,
            peers_seen,
            peers_dead,
            eta_human,
            fetched_bytes,
        ) = match &s.live {
            Some(live) => (
                live.download_speed.as_bytes(),
                live.upload_speed.as_bytes(),
                live.snapshot.peer_stats.live,
                live.snapshot.peer_stats.connecting,
                live.snapshot.peer_stats.queued,
                live.snapshot.peer_stats.seen,
                live.snapshot.peer_stats.dead,
                live.time_remaining.as_ref().map(|t| t.to_string()),
                live.snapshot.fetched_bytes,
            ),
            None => (0, 0, 0, 0, 0, 0, 0, None, 0),
        };
        DownloadStats {
            id: self.id(),
            info_hash: self.info_hash_hex(),
            name: self.name(),
            state,
            progress_bytes: s.progress_bytes,
            fetched_bytes,
            total_bytes: s.total_bytes,
            uploaded_bytes: s.uploaded_bytes,
            file_progress: s.file_progress,
            finished: s.finished,
            live: matches!(s.state, TorrentStatsState::Live),
            download_speed,
            upload_speed,
            peers_live,
            peers_connecting,
            peers_queued,
            peers_seen,
            peers_dead,
            eta_human,
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

    /// Indices des fichiers selectionnes pour le telechargement
    /// (`None` = tous — `selected_files` Python / `only_files` rqbit).
    pub fn only_files(&self) -> Option<Vec<usize>> {
        self.inner.only_files()
    }

    /// Nombre de fichiers du torrent (metadonnees requises).
    pub fn file_count(&self) -> Option<usize> {
        Some(self.inner.metadata.load().as_ref()?.file_infos.len())
    }

    /// Nombre total de pieces (`tdef.torrent_info.num_pieces()`
    /// Python — disponible meme en pause, tant que le metainfo est
    /// resolu).
    pub fn total_pieces(&self) -> Option<u32> {
        let md = self.inner.metadata.load();
        Some(md.as_ref()?.info.lengths().total_pieces())
    }

    /// Fichiers du contenu (metadonnees resolues ; `None` tant qu'un
    /// magnet n'a pas recu son info — comme `download.tdef` Python).
    /// `progress` = fraction d'octets deja presentes (`file_progress`
    /// rqbit).
    pub fn files(&self) -> Option<Vec<DownloadFile>> {
        let md = self.inner.metadata.load();
        let md = md.as_ref()?;
        let progress = self.inner.stats().file_progress;
        Some(
            md.file_infos
                .iter()
                .enumerate()
                .map(|(index, fi)| {
                    let have = progress.get(index).copied().unwrap_or(0);
                    DownloadFile {
                        index,
                        name: fi.relative_filename.display().to_string(),
                        length: fi.len,
                        progress: if fi.len == 0 {
                            1.0
                        } else {
                            (have as f64 / fi.len as f64).clamp(0.0, 1.0)
                        },
                    }
                })
                .collect(),
        )
    }

    /// URLs des trackers du torrent (`announce`/`announce-list`,
    /// `HashSet` libtorrent-equivalent) **plus** ceux ajoutes a chaud
    /// par `add_tracker`.
    pub fn trackers(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .inner
            .shared()
            .trackers
            .iter()
            .map(|u| u.to_string())
            .collect();
        if let Ok(extra) = self.extra_trackers.lock() {
            for t in extra.iter() {
                if !out.iter().any(|e| e == t) {
                    out.push(t.clone());
                }
            }
        }
        out
    }

    /// `add_peer` Python (`TriblerTunnelCommunity.on_e2e_finished` /
    /// `readd_bittorrent_peers`) : injecte une adresse de pair dans le
    /// torrent vivant — pour les pairs caches, l'adresse est le
    /// relais loopback de `udp_relay::dial` (ou l'IPv4 factice du
    /// circuit cote SOCKS5). Un pair deja connu a l'etat `Dead` est
    /// releve immediatement (requeue, sinon le backoff exponentiel
    /// interne 10 s -> 1 h figeait la source). No-op si le torrent
    /// n'est pas `live`.
    /// Retourne `true` si le pair etait nouveau ou releve de `Dead`.
    pub fn add_peer(&self, addr: std::net::SocketAddr) -> bool {
        self.inner
            .live()
            .and_then(|live| live.add_peer_if_not_seen(addr).ok())
            .unwrap_or(false)
    }

    /// Enregistre un tracker additionnel (equivalent de
    /// `PUT /downloads/{ih}/trackers` Python ; effectif a la prochaine
    /// session pour rqbit qui ne reannonce pas a chaud).
    pub fn add_tracker(&self, url: &str) {
        if let Ok(mut extra) = self.extra_trackers.lock() {
            if !extra.iter().any(|e| e == url) {
                extra.push(url.to_string());
            }
        }
    }

    /// Retire un tracker ajoute a chaud (no-op s'il vient de la
    /// source `.torrent`/magnet — ceux-ci sont filtres a la
    /// recreation par `removed_trackers`, cf. `onionbit-core`).
    pub fn remove_extra_tracker(&self, url: &str) {
        if let Ok(mut extra) = self.extra_trackers.lock() {
            extra.retain(|e| e != url);
        }
    }

    /// Octets du `.torrent` source (metadonnees resolues).
    pub fn torrent_bytes(&self) -> Option<bytes::Bytes> {
        self.inner
            .metadata
            .load()
            .as_ref()
            .map(|m| m.torrent_bytes.clone())
    }

    /// Ouvre un flux de lecture d'un fichier du torrent
    /// (`GET /downloads/{ih}/stream/{i}`).
    ///
    /// `FileStream` de rqbit n'est pas reexporte publiquement : on le
    /// pompe dans un canal de chunks (256 Kio) consomme par le handler
    /// HTTP — equivalent du streaming de `stream()` Python.
    pub async fn stream_file(&self, file_index: usize) -> crate::Result<DownloadStream> {
        self.stream_file_from(file_index, 0).await
    }

    /// Variante avec offset de depart (`start` de l'endpoint de
    /// streaming Python — seek avant lecture).
    pub async fn stream_file_from(
        &self,
        file_index: usize,
        start: u64,
    ) -> crate::Result<DownloadStream> {
        let stream = self
            .inner
            .clone()
            .stream(file_index)
            .await
            .map_err(|e| crate::BtError::Engine(e.to_string()))?;
        let len = stream.len();
        let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAP);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncSeek};
            let mut fs = std::pin::pin!(stream);
            if start > 0
                && AsyncSeek::start_seek(fs.as_mut(), std::io::SeekFrom::Start(start)).is_err()
            {
                return;
            }
            loop {
                let mut buf = vec![0u8; STREAM_CHUNK];
                match fs.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        if tx.send(Ok(bytes::Bytes::from(buf))).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }
        });
        Ok(DownloadStream { length: len, rx })
    }
}

/// Taille d'un chunk de streaming (256 Kio — borne de buffer par
/// client HTTP).
const STREAM_CHUNK: usize = 262_144;

/// Nombre de chunks en vol par flux (backpressure mpsc).
const STREAM_CHANNEL_CAP: usize = 4;

/// Flux d'un fichier de torrent en cours (stream HTTP).
pub struct DownloadStream {
    /// Taille totale du fichier.
    pub length: u64,
    /// Receveur de chunks.
    pub rx: tokio::sync::mpsc::Receiver<std::io::Result<bytes::Bytes>>,
}

/// Fichier d'un telechargement (DTO interne, sans `serde` dans ce
/// crate — la couche API le serialise).
#[derive(Debug, Clone)]
pub struct DownloadFile {
    /// Index dans la liste des fichiers du torrent.
    pub index: usize,
    /// Chemin relatif dans le torrent.
    pub name: String,
    /// Taille en octets.
    pub length: u64,
    /// Fraction d'octets deja telecharges (0.0..1.0).
    pub progress: f64,
}
