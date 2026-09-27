//! Configuration de la session coeur.
//!
//! Regroupe tous les reglages d'orchestration (les reglages du moteur
//! BitTorrent vivent dans `tribler_bittorrent::EngineConfig`, ceux du
//! daemon HTTP dans `tribler-daemon`).

use std::path::PathBuf;

/// Intervalle par defaut d'emission des notifications de progression
/// (le Notifier Python de Tribler emet `torrents_status_update` selon
/// un rythme similaire).
pub const DEFAULT_PROGRESS_INTERVAL_MS: u64 = 1000;

/// Configuration de [`crate::CoreSession`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Repertoire d'etat du daemon (base SQLite, resume, cles).
    pub state_dir: PathBuf,
    /// Repertoire de telechargement par defaut.
    pub downloads_dir: PathBuf,
    /// Nom du fichier de base (relatif a `state_dir`).
    pub db_filename: String,
    /// Intervalle de publication des stats de progression.
    pub progress_interval_ms: u64,
    /// Politique anti-SSRF appliquee aux URI distantes (`http(s)`)
    /// ajoutees comme telechargements : resolution DNS puis refus de
    /// toute adresse non autorisee. `strict` en production,
    /// `permissive` en test offline.
    pub ip_policy: tribler_network_policy::IpPolicy,
    /// Repertoire surveille par le watch folder (`None` = desactive,
    /// `watch_folder/directory` Python).
    pub watch_folder_dir: Option<PathBuf>,
    /// Intervalle de scan du watch folder (ms).
    pub watch_folder_interval_ms: u64,
    /// Flux RSS surveilles (`rss` Python — liste vide = desactive).
    pub rss_urls: Vec<String>,
    /// Active le controle periodique de sante des torrents
    /// (`torrent_checker`).
    pub enable_torrent_checker: bool,
    /// Intervalle de controle du torrent checker (ms).
    pub torrent_checker_interval_ms: u64,
    /// Stack IPv8 de session (decouverte, content discovery, tunnels
    /// anonymes — etape 15). `enabled = false` par defaut.
    pub ipv8: crate::ipv8_stack::Ipv8Config,
    /// Configuration du moteur BitTorrent sous-jacent.
    pub engine: tribler_bittorrent::EngineConfig,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".tribler"),
            downloads_dir: PathBuf::from("downloads"),
            db_filename: "tribler.db".into(),
            progress_interval_ms: DEFAULT_PROGRESS_INTERVAL_MS,
            ip_policy: tribler_network_policy::IpPolicy::strict(),
            watch_folder_dir: None,
            watch_folder_interval_ms: crate::services::watch_folder::DEFAULT_CHECK_INTERVAL
                .as_millis() as u64,
            rss_urls: Vec::new(),
            enable_torrent_checker: true,
            torrent_checker_interval_ms: 10_000,
            ipv8: crate::ipv8_stack::Ipv8Config::default(),
            engine: tribler_bittorrent::EngineConfig::default(),
        }
    }
}

impl CoreConfig {
    /// Chemin complet du fichier de base.
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join(&self.db_filename)
    }

    /// Configuration isolee pour les tests : aucun trafic sortant,
    /// repertoire temporaire, base en memoire geree par l'appelant.
    pub fn offline(state_dir: PathBuf) -> Self {
        Self {
            engine: tribler_bittorrent::EngineConfig::offline(state_dir.join("downloads")),
            downloads_dir: state_dir.join("downloads"),
            state_dir,
            db_filename: "tribler.db".into(),
            progress_interval_ms: DEFAULT_PROGRESS_INTERVAL_MS,
            ip_policy: tribler_network_policy::IpPolicy::permissive(),
            watch_folder_dir: None,
            watch_folder_interval_ms: crate::services::watch_folder::DEFAULT_CHECK_INTERVAL
                .as_millis() as u64,
            rss_urls: Vec::new(),
            enable_torrent_checker: false,
            torrent_checker_interval_ms: 10_000,
            ipv8: crate::ipv8_stack::Ipv8Config::default(),
        }
    }
}
