//! Bus d'evenements interne (equivalent de `tribler.core.notifier`).
//!
//! Un `tokio::sync::broadcast` distribue les [`Notification`] aux
//! abonnes : `tribler-api` (SSE), les services internes
//! (torrent_checker, content_discovery), les logs.
//!
//! La capacite du canal est bornee ([`CAPACITY`]) : un abonne lent
//! recevra `RecvError::Lagged` et resynchronisera — comme le Notifier
//! Python, on privilegie la non-blocage des emetteurs.

use tokio::sync::broadcast;

/// Capacite du canal de broadcast (nombre d'evenements tamponnes par
/// abonne avant lag).
pub const CAPACITY: usize = 256;

/// Evenement interne du daemon.
///
/// Les variantes portent des donnees deja serialisables/legères ; la
/// traduction vers les topics SSE Python
/// (`Notification.*` de `tribler.core.restapi`) se fait dans
/// `tribler-api`.
#[derive(Debug, Clone)]
pub enum Notification {
    /// Progression d'un telechargement (emission periodique agregee).
    DownloadProgress(tribler_bittorrent::DownloadStats),
    /// Telechargement termine (toutes les pieces).
    DownloadFinished {
        /// Info-hash hex.
        infohash: String,
        /// Nom du contenu.
        name: Option<String>,
    },
    /// Etat d'un telechargement a change (pause/reprise/erreur).
    DownloadStateChanged {
        /// Info-hash hex.
        infohash: String,
        /// Nouvel etat.
        state: tribler_bittorrent::DownloadState,
    },
    /// Nouvelle metadonnee de torrent ingeree en base
    /// (`new_torrent_metadata_created` cote Python).
    TorrentMetadataCreated {
        /// Info-hash hex.
        infohash: String,
        /// Titre.
        title: String,
    },
    /// Sante d'un torrent mise a jour (`torrent_health_finished` /
    /// `remote_torrent_health_update` cote Python).
    TorrentHealthUpdated {
        /// Info-hash hex.
        infohash: String,
        /// Seeders observes.
        seeders: i64,
        /// Leechers observes.
        leechers: i64,
    },
    /// Le daemon a termine son demarrage.
    SessionStarted,
    /// Le daemon s'arrete.
    SessionStopping,
}

/// Bus de notifications. `Clone` : chaque service detient un
/// emetteur partage.
#[derive(Clone)]
pub struct Notifier {
    tx: broadcast::Sender<Notification>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier {
    /// Cree un bus vide.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Emet un evenement (ignore s'il n'y a aucun abonne).
    pub fn notify(&self, n: Notification) {
        // send() n'echoue que sans abonnes — non bloquant par design.
        let _ = self.tx.send(n);
    }

    /// Cree un abonne recevant les prochains evenements.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.tx.subscribe()
    }
}
