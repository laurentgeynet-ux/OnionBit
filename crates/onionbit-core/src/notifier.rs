//! Bus d'evenements interne (equivalent de `tribler.core.notifier`).
//!
//! Un `tokio::sync::broadcast` distribue les [`Notification`] aux
//! abonnes : `onionbit-api` (SSE), les services internes
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
/// `onionbit-api`.
#[derive(Debug, Clone)]
pub enum Notification {
    /// Progression d'un telechargement (emission periodique agregee).
    DownloadProgress(onionbit_bittorrent::DownloadStats),
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
        state: onionbit_bittorrent::DownloadState,
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
    /// `tribler_shutdown_state` : progression de la sequence d'arret
    /// (messages `Shutting down ...` de `Session.shutdown()` Python).
    ShutdownState {
        /// Message de phase.
        state: String,
    },
    /// `remote_query_results` : resultats d'un `remote_select`
    /// (`send_search_request` -> `processing_callback` Python).
    RemoteQueryResults {
        /// Texte de recherche (`txt_filter`).
        query: String,
        /// `to_simple_dict()` des objets nouveaux.
        results: Vec<serde_json::Value>,
        /// UUID de la requete (`request_uuid`).
        uuid: String,
        /// Pair repondant (`hexlify(peer.mid)`).
        peer: String,
    },
    /// `local_query_results` : resultats de la recherche locale
    /// (`local_search` du `database_endpoint` Python).
    LocalQueryResults {
        /// Texte de recherche (`fts_text`).
        query: String,
        /// `to_simple_dict()` des resultats.
        results: Vec<serde_json::Value>,
    },
    /// `tunnel_removed` : objet de routage tunnel detruit
    /// (`Notification.circuit_removed` pyipv8).
    TunnelRemoved {
        /// `circuit_id` de l'objet.
        circuit_id: u32,
        /// Classe (`"Circuit"`, `"RelayRoute"`, `"TunnelExitSocket"`).
        circuit_class: String,
        /// Octets montants.
        bytes_up: u64,
        /// Octets descendants.
        bytes_down: u64,
        /// Duree de vie en secondes.
        uptime_secs: f64,
        /// Contexte (`additional_info` Python).
        additional_info: String,
    },
    /// `low_space` : espace disque faible sur le dossier de
    /// telechargement (`disk_usage_data` = `total/used/free`).
    LowSpace {
        /// `{"total": .., "used": .., "free": ..}` (mirroir
        /// `shutil.disk_usage` du `statistics_endpoint` Python).
        disk_usage_data: serde_json::Value,
    },
    /// `tribler_exception` : exception non rattrapee remontee au GUI
    /// (`on_tribler_exception` Python — traceback formate).
    TriblerException {
        /// Texte d'erreur (type + message Python).
        error: String,
    },
    /// `report_config_error` : erreur de configuration detectee au
    /// demarrage ou a la relecture.
    ReportConfigError {
        /// Texte d'erreur.
        error: String,
    },
    /// `ask_add_download` : `ask_download_settings` actif — le GUI
    /// doit ouvrir la boite de reglages au lieu d'ajouter.
    AskAddDownload {
        /// URI demandee.
        uri: String,
    },
    /// `tribler_new_version` : une version plus recente existe.
    TriblerNewVersion {
        /// Chaine de version distante.
        version: String,
    },
    /// `settings_changed` : `POST /api/settings` a modifie et persiste
    /// la configuration — les clients doivent recharger l'arbre
    /// (boucle multi-clients : l'editeur avance d'un client
    /// resynchronise les sections dediees des autres).
    SettingsChanged,
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
