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

/// Backend concret d'un [`Download`] — polymorphe rqbit/bitdaemon
/// (ONIONBIT-1). La surface publique de `Download` est inchangee ;
/// chaque methode dispatche sur la variante.
#[derive(Clone)]
pub(crate) enum DownloadInner {
    /// Backend `librqbit` vendored (reference actuelle).
    Rqbit(Arc<librqbit::ManagedTorrent>),
    /// Backend `bitdaemon` (port libtorrent).
    Bd(BdTorrent),
}

/// Handle d'un telechargement gere par la session.
///
/// Clone leger (`Arc` interne) : peut etre partage entre les couches.
#[derive(Clone)]
pub struct Download {
    pub(crate) inner: DownloadInner,
    /// Trackers ajoutes a chaud via l'API (`PUT /downloads/{ih}/trackers`).
    /// rqbit ne permet pas de muter `shared.trackers` apres
    /// l'initialisation ; on les fusionne dans `trackers()` et ils
    /// sont persistes dans la ligne `downloads` par le core.
    pub(crate) extra_trackers: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Handle d'un torrent du backend `bitdaemon` : `TorrentHandle` de la
/// session partagee + octets `.torrent` sources conserves a l'ajout
/// (rqbit les garde en `metadata.torrent_bytes` ; bitdaemon expose le
/// `info_section`, pas le fichier entier).
#[derive(Clone)]
pub(crate) struct BdTorrent {
    /// Handle session bitdaemon (`SessionHandle::find_torrent`).
    pub handle: bitdaemon_session::session_handle::TorrentHandle,
    /// Handle de session partage — acces au `Mutex<Session>` pour les
    /// snapshots bruts (`peer_stats`).
    pub session: bitdaemon_session::session_handle::SessionHandle,
    /// Info-hash v1 (20 octets) — cle du `find_torrent`.
    pub info_hash: [u8; 20],
    /// Octets du `.torrent` a l'ajout (`None` pour un magnet).
    pub torrent_bytes: Option<bytes::Bytes>,
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
    /// Identifiant numerique interne (`0` sous bitdaemon — le port
    /// n'a pas d'id numerique, l'info-hash est la cle).
    pub fn id(&self) -> usize {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.id(),
            DownloadInner::Bd(_) => 0,
        }
    }

    /// Info-hash v1 (20 octets).
    pub fn info_hash(&self) -> onionbit_crypto::hash::InfoHashV1 {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.info_hash().0,
            DownloadInner::Bd(t) => t.info_hash,
        }
    }

    /// Info-hash v1 en hexadecimal minuscule.
    pub fn info_hash_hex(&self) -> String {
        onionbit_crypto::hash::to_hex(&self.info_hash())
    }

    /// Nom du torrent si les metadonnees sont deja connues
    /// (`None` tant qu'un magnet n'a pas recu ses metadonnees).
    pub fn name(&self) -> Option<String> {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.name(),
            DownloadInner::Bd(t) => t
                .handle
                .torrent_file()
                .map(|ti| String::from_utf8_lossy(ti.name()).into_owned())
                .or_else(|| t.handle.status().map(|s| s.name).filter(|n| !n.is_empty())),
        }
    }

    /// Repertoire effectif d'ecriture des fichiers.
    pub fn output_folder(&self) -> std::path::PathBuf {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.output_folder().to_path_buf(),
            DownloadInner::Bd(t) => t
                .handle
                .status()
                .map(|s| std::path::PathBuf::from(s.save_path))
                .unwrap_or_default(),
        }
    }

    /// Flag `private` du metainfo (DHT/PEX/LSD desactives —
    /// pairs uniquement via le tracker).
    pub fn is_private(&self) -> bool {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.with_metadata(|m| m.info.info().private).unwrap_or(false),
            DownloadInner::Bd(t) => t
                .handle
                .torrent_file()
                .map(|ti| ti.is_private())
                .unwrap_or(false),
        }
    }

    /// Le torrent est-il en pause ?
    pub fn is_paused(&self) -> bool {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.is_paused(),
            DownloadInner::Bd(t) => t.handle.status().map(|s| s.paused).unwrap_or(true),
        }
    }

    /// Instantane de l'etat courant.
    pub fn stats(&self) -> DownloadStats {
        match &self.inner {
            DownloadInner::Rqbit(t) => self.stats_rqbit(t),
            DownloadInner::Bd(t) => self.stats_bd(t),
        }
    }

    /// Stats du backend `bitdaemon` (`TorrentStatus` -> `DownloadStats`).
    fn stats_bd(&self, t: &BdTorrent) -> DownloadStats {
        let Some(s) = t.handle.status() else {
            return DownloadStats {
                id: 0,
                info_hash: self.info_hash_hex(),
                name: self.name(),
                state: DownloadState::Stopped,
                progress_bytes: 0,
                fetched_bytes: 0,
                total_bytes: 0,
                uploaded_bytes: 0,
                file_progress: Vec::new(),
                finished: false,
                live: false,
                download_speed: 0,
                upload_speed: 0,
                peers_live: 0,
                peers_connecting: 0,
                peers_queued: 0,
                peers_seen: 0,
                peers_dead: 0,
                eta_human: None,
                error: None,
            };
        };
        use bitdaemon_session::status::TorrentState as TS;
        let state = if !s.error.is_empty() {
            DownloadState::Error
        } else if s.paused {
            DownloadState::Paused
        } else {
            match s.state {
                TS::CheckingResumeData | TS::CheckingFiles | TS::QueuedForChecking => {
                    DownloadState::Checking
                }
                TS::DownloadingMetadata => DownloadState::Initializing,
                TS::Downloading => DownloadState::Downloading,
                TS::Finished | TS::Seeding => DownloadState::Seeding,
            }
        };
        let file_progress: Vec<u64> = t
            .handle
            .file_progress()
            .into_iter()
            .map(|v| v.max(0) as u64)
            .collect();
        let eta = if s.download_rate > 0 && s.total_wanted > s.total_done {
            let secs = (s.total_wanted - s.total_done) / s.download_rate;
            Some(format_duration(secs.max(0) as u64))
        } else {
            None
        };
        DownloadStats {
            id: 0,
            info_hash: self.info_hash_hex(),
            name: if s.name.is_empty() {
                self.name()
            } else {
                Some(s.name)
            },
            state,
            progress_bytes: s.total_done.max(0) as u64,
            fetched_bytes: s.total_download.max(0) as u64,
            total_bytes: s.total_wanted.max(0) as u64,
            uploaded_bytes: s.total_upload.max(0) as u64,
            file_progress,
            finished: s.finished,
            live: s.num_peers > 0 || s.download_rate > 0 || s.upload_rate > 0,
            download_speed: s.download_rate.max(0) as u64,
            upload_speed: s.upload_rate.max(0) as u64,
            peers_live: s.num_peers as u32,
            peers_connecting: 0,
            peers_queued: 0,
            peers_seen: s.num_candidates as u32,
            peers_dead: 0,
            eta_human: eta,
            error: (!s.error.is_empty()).then_some(s.error),
        }
    }

    /// Stats du backend `librqbit` (corps historique).
    fn stats_rqbit(&self, inner: &Arc<librqbit::ManagedTorrent>) -> DownloadStats {
        let s = inner.stats();
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
        match &self.inner {
            DownloadInner::Rqbit(t) => t
                .wait_until_initialized()
                .await
                .map_err(|e| crate::BtError::Engine(e.to_string())),
            // bitdaemon : metadonnees resolues quand `torrent_file`
            // repond — sondage (pas de waiter upstream).
            DownloadInner::Bd(t) => {
                while t.handle.is_valid() {
                    if t.handle.torrent_file().is_some() {
                        return Ok(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                Err(crate::BtError::Engine("torrent supprime".into()))
            }
        }
    }

    /// Attend la fin du telechargement (toutes les pieces).
    pub async fn wait_completed(&self) -> crate::Result<()> {
        match &self.inner {
            DownloadInner::Rqbit(t) => t
                .wait_until_completed()
                .await
                .map_err(|e| crate::BtError::Engine(e.to_string())),
            DownloadInner::Bd(t) => {
                while t.handle.is_valid() {
                    if t.handle.status().is_some_and(|s| s.finished) {
                        return Ok(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Err(crate::BtError::Engine("torrent supprime".into()))
            }
        }
    }

    /// Indices des fichiers selectionnes pour le telechargement
    /// (`None` = tous — `selected_files` Python / `only_files` rqbit).
    pub fn only_files(&self) -> Option<Vec<usize>> {
        match &self.inner {
            DownloadInner::Rqbit(t) => t.only_files(),
            // bitdaemon : `file_priorities` — `DONT_DOWNLOAD` (0)
            // exclus ; `None` quand tous sont voulus.
            DownloadInner::Bd(t) => {
                let prios = t.handle.file_priorities();
                if prios.is_empty() {
                    return None;
                }
                let wanted: Vec<usize> = prios
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        **p != bitdaemon_common::types::DownloadPriority::DONT_DOWNLOAD
                    })
                    .map(|(i, _)| i)
                    .collect();
                (wanted.len() != prios.len()).then_some(wanted)
            }
        }
    }

    /// Nombre de fichiers du torrent (metadonnees requises).
    pub fn file_count(&self) -> Option<usize> {
        match &self.inner {
            DownloadInner::Rqbit(t) => Some(t.metadata.load().as_ref()?.file_infos.len()),
            DownloadInner::Bd(t) => t
                .handle
                .torrent_file()
                .map(|ti| ti.num_files().max(0) as usize),
        }
    }

    /// Nombre total de pieces (`tdef.torrent_info.num_pieces()`
    /// Python — disponible meme en pause, tant que le metainfo est
    /// resolu).
    pub fn total_pieces(&self) -> Option<u32> {
        match &self.inner {
            DownloadInner::Rqbit(t) => {
                let md = t.metadata.load();
                Some(md.as_ref()?.info.lengths().total_pieces())
            }
            DownloadInner::Bd(t) => t.handle.status().map(|s| s.num_pieces.max(0) as u32),
        }
    }

    /// Fichiers du contenu (metadonnees resolues ; `None` tant qu'un
    /// magnet n'a pas recu son info — comme `download.tdef` Python).
    /// `progress` = fraction d'octets deja presentes (`file_progress`
    /// rqbit).
    pub fn files(&self) -> Option<Vec<DownloadFile>> {
        match &self.inner {
            DownloadInner::Rqbit(t) => {
                let md = t.metadata.load();
                let md = md.as_ref()?;
                let progress = t.stats().file_progress;
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
            DownloadInner::Bd(t) => {
                let ti = t.handle.torrent_file()?;
                let fs = ti.files();
                let progress = t.handle.file_progress();
                Some(
                    (0..fs.num_files())
                        .map(|i| {
                            let idx = bitdaemon_common::types::FileIndex(i);
                            let len = fs.file_size(idx).max(0) as u64;
                            let have = progress.get(i as usize).copied().unwrap_or(0).max(0) as u64;
                            DownloadFile {
                                index: i as usize,
                                name: String::from_utf8_lossy(fs.file_name(idx)).into_owned(),
                                length: len,
                                progress: if len == 0 {
                                    1.0
                                } else {
                                    (have as f64 / len as f64).clamp(0.0, 1.0)
                                },
                            }
                        })
                        .collect(),
                )
            }
        }
    }

    /// URLs des trackers du torrent (`announce`/`announce-list`,
    /// `HashSet` libtorrent-equivalent) **plus** ceux ajoutes a chaud
    /// par `add_tracker`.
    pub fn trackers(&self) -> Vec<String> {
        let mut out: Vec<String> = match &self.inner {
            DownloadInner::Rqbit(t) => t.shared().trackers.iter().map(|u| u.to_string()).collect(),
            DownloadInner::Bd(t) => t
                .handle
                .trackers()
                .into_iter()
                .map(|(url, _)| url)
                .collect(),
        };
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
        match &self.inner {
            DownloadInner::Rqbit(t) => t
                .live()
                .and_then(|live| live.add_peer_if_not_seen(addr).ok())
                .unwrap_or(false),
            // `connect_peer` — candidat de dial immediat ; retourne
            // `true` quand le torrent existe (le dedup est interne).
            DownloadInner::Bd(t) => {
                if !t.handle.is_valid() {
                    return false;
                }
                t.handle.connect_peer(addr);
                true
            }
        }
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
        match &self.inner {
            DownloadInner::Rqbit(t) => t.metadata.load().as_ref().map(|m| m.torrent_bytes.clone()),
            // bitdaemon expose `info_section`, pas le fichier entier
            // — les octets sont ceux de l'ajout (`None` pour magnet).
            DownloadInner::Bd(t) => t.torrent_bytes.clone(),
        }
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
        // bitdaemon : lecture directe du fichier sur disque — les
        // pieces manquantes rendent les octets presents de leur
        // portion ecrite (sparse : des zeros, comme un mmap libtorrent).
        if let DownloadInner::Bd(t) = &self.inner {
            return self.stream_bd(t, file_index, start);
        }
        let DownloadInner::Rqbit(inner) = &self.inner else {
            unreachable!()
        };
        let stream = inner
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

    /// Streaming backend `bitdaemon` : le fichier est pompe depuis le
    /// disque (`save_path` + chemin relatif du `FileStorage`) dans le
    /// meme canal de chunks que la variante rqbit.
    fn stream_bd(
        &self,
        t: &BdTorrent,
        file_index: usize,
        start: u64,
    ) -> crate::Result<DownloadStream> {
        let ti = t
            .handle
            .torrent_file()
            .ok_or_else(|| crate::BtError::Engine("metadonnees non resolues".into()))?;
        let status = t
            .handle
            .status()
            .ok_or_else(|| crate::BtError::NotFound(self.info_hash_hex()))?;
        let fs = ti.files();
        let idx = bitdaemon_common::types::FileIndex(file_index as i32);
        if file_index as i32 >= fs.num_files() {
            return Err(crate::BtError::NotFound(format!("file {file_index}")));
        }
        let length = fs.file_size(idx).max(0) as u64;
        let mut path = std::path::PathBuf::from(status.save_path);
        path.push(String::from_utf8_lossy(fs.file_name(idx)).into_owned());
        let file = std::fs::File::open(&path)
            .map_err(|e| crate::BtError::Engine(format!("{} — {e}", path.display())))?;
        let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAP);
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = file;
            if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
                return;
            }
            loop {
                let mut buf = vec![0u8; STREAM_CHUNK];
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        if tx.blocking_send(Ok(bytes::Bytes::from(buf))).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        break;
                    }
                }
            }
        });
        Ok(DownloadStream { length, rx })
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

/// Formatte une duree en texte humain (`3m 20s`) — ETA des stats
/// `bitdaemon` (rqbit fournit `time_remaining` deja formate).
fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
