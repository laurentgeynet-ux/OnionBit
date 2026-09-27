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
        }
    }
}
