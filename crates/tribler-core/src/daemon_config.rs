//! Configuration persistée du daemon — `state_dir/configuration.json`.
//!
//! Équivalent de `tribler.tribler_config` (`TriblerConfig` +
//! `TriblerConfigManager`) : arbre de réglages complet avec défauts,
//! chargé au démarrage du daemon, réécrit après chaque `POST
//! /api/settings` (merge récursif, fidèle à
//! `SettingsEndpoint._recursive_merge_settings` qui fait `set("a/b", v)`
//! par feuille — équivalent à une fusion profonde des objets JSON).
//!
//! Les clés inconnues sont préservées (`extra`, serde `flatten`) comme
//! le dict Python les conserve.
//!
//! Écart assumé : `libtorrent/download_defaults/saveas` vide (défaut)
//! est résolu en `<state_dir>/downloads` au lieu de `~/Downloads` —
//! le daemon n'écrit jamais hors de son répertoire d'état sans
//! configuration explicite.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Nom du fichier de configuration dans `state_dir`.
pub const CONFIG_FILENAME: &str = "configuration.json";

/// Merge JSON profond (`_recursive_merge_settings` Python) : les objets
/// se fusionnent clé par clé, toute autre valeur remplace.
fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                deep_merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Section `api` — écoute HTTP(S) du plan de contrôle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Clé API hex (générée au premier run — `ApiKeyMiddleware`).
    pub key: String,
    /// Expose l'API HTTP.
    pub http_enabled: bool,
    /// Hôte d'écoute (loopback uniquement — politique réseau).
    pub http_host: String,
    /// Port demandé (0 = aléatoire, publié dans `http_port_running`).
    pub http_port: u16,
    /// HTTPS désactivé par défaut (comme Python).
    pub https_enabled: bool,
    /// Hôte HTTPS.
    pub https_host: String,
    /// Port HTTPS.
    pub https_port: u16,
    /// Certificat PEM pour HTTPS.
    pub https_certfile: String,
    /// Port HTTP réellement lié (écrit par le daemon au runtime).
    pub http_port_running: u16,
    /// Port HTTPS réellement lié.
    pub https_port_running: u16,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            key: String::new(),
            http_enabled: true,
            http_host: "127.0.0.1".into(),
            http_port: 0,
            https_enabled: false,
            https_host: "127.0.0.1".into(),
            https_port: 0,
            https_certfile: "https_certfile".into(),
            http_port_running: 0,
            https_port_running: 0,
        }
    }
}

/// Interface d'écoute IPv8 (`ipv8/interfaces` pyipv8).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8Interface {
    /// Nom de l'interface (`UDPIPv4`, `UDPIPv6`).
    pub interface: String,
    /// Adresse de bind.
    pub ip: String,
    /// Port UDP.
    pub port: u16,
    /// Threads de traitement (optionnel pyipv8).
    pub worker_threads: Option<u32>,
}

impl Default for Ipv8Interface {
    fn default() -> Self {
        Self {
            interface: "UDPIPv4".into(),
            ip: "0.0.0.0".into(),
            port: crate::ipv8_stack::DEFAULT_IPV8_PORT,
            worker_threads: None,
        }
    }
}

/// Section `ipv8/bootstrap` : `override` remplace la liste d'amorçage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8Bootstrap {
    /// Noeuds d'amorçage explicites (remplace la liste par défaut si
    /// non vide — comme `bootstrap.override` pyipv8).
    #[serde(rename = "override")]
    pub override_peers: Vec<String>,
}

/// Section `ipv8` — moteur overlay (découverte, communities).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8FileConfig {
    /// Active la stack IPv8.
    pub enabled: bool,
    /// Interfaces d'écoute UDP (défaut : IPv4 8090 + IPv6 8091, pyipv8).
    pub interfaces: Vec<Ipv8Interface>,
    /// Liste d'amorçage personnalisée.
    pub bootstrap: Ipv8Bootstrap,
    /// Intervalle des stratégies de découverte (s).
    pub walker_interval: f64,
    /// Niveau de log (DEBUG/INFO/WARNING/ERROR).
    pub logger_level: String,
    /// Clés/communities additionnelles non portées — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for Ipv8FileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interfaces: vec![
                Ipv8Interface::default(),
                Ipv8Interface {
                    interface: "UDPIPv6".into(),
                    ip: "::".into(),
                    port: crate::ipv8_stack::DEFAULT_IPV8_PORT + 1,
                    worker_threads: None,
                },
            ],
            bootstrap: Ipv8Bootstrap::default(),
            walker_interval: 0.5,
            logger_level: "INFO".into(),
            extra: serde_json::Map::new(),
        }
    }
}

/// `libtorrent/download_defaults` — réglages par défaut des ajouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadDefaultsConfig {
    /// Anonymat activé par défaut pour les nouveaux téléchargements.
    pub anonymity_enabled: bool,
    /// Nombre de sauts par défaut (0 = non anonyme, max 3).
    pub number_hops: u32,
    /// Seeding sûr — requis si `number_hops > 0`.
    pub safeseeding_enabled: bool,
    /// Dossier de destination (`""` = `<state_dir>/downloads` — écart
    /// assumé vs `~/Downloads` Python, cf. note du module).
    pub saveas: String,
    /// Mode de seed : `forever` / `never` / `ratio` / `time`.
    pub seeding_mode: String,
    /// Ratio cible si `seeding_mode == "ratio"`.
    pub seeding_ratio: f64,
    /// Durée de seed cible (s) si `seeding_mode == "time"`.
    pub seeding_time: f64,
    /// Téléchargement de channel.
    pub channel_download: bool,
    /// Ajoute le téléchargement à un channel.
    pub add_download_to_channel: bool,
    /// Fichier de trackers par défaut.
    pub trackers_file: String,
    /// URL de synchronisation de `trackers_file`.
    pub trackers_file_sync_url: String,
    /// Sauvegarde des .torrent dans ce dossier.
    pub torrent_folder: String,
    /// auto_managed par défaut (file d'attente).
    pub auto_managed: bool,
    /// Déplacement après complétion.
    pub completed_dir: String,
}

impl Default for DownloadDefaultsConfig {
    fn default() -> Self {
        Self {
            anonymity_enabled: true,
            number_hops: 1,
            safeseeding_enabled: true,
            saveas: String::new(),
            seeding_mode: "forever".into(),
            seeding_ratio: 2.0,
            seeding_time: 60.0,
            channel_download: false,
            add_download_to_channel: false,
            trackers_file: String::new(),
            trackers_file_sync_url: String::new(),
            torrent_folder: String::new(),
            auto_managed: false,
            completed_dir: String::new(),
        }
    }
}

/// Section `libtorrent` — réglages de la session BitTorrent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibtorrentConfig {
    /// Ports SOCKS5 par lane d'anonymat (sous `download_defaults` dans
    /// Tribler récent — conservé ici pour compatibilité ascendante).
    pub socks_listen_ports: Vec<u16>,
    /// Interface d'écoute.
    pub listen_interface: String,
    /// Port d'écoute (0 = aléatoire).
    pub port: u16,
    /// Interface IPv6 ("" = désactivée).
    pub listen_interface_v6: String,
    /// Port IPv6.
    pub port_v6: u16,
    /// Type de proxy (enum libtorrent : 0 aucun, 2/3 socks5, 4/5 http).
    pub proxy_type: i64,
    /// Proxy `host:port`.
    pub proxy_server: String,
    /// Auth proxy `user:pass`.
    pub proxy_auth: String,
    /// Connexions max par téléchargement (-1 = illimité).
    pub max_connections_download: i64,
    /// Limite globale download (o/s, 0 = illimité).
    pub max_download_rate: u64,
    /// Limite globale upload (o/s, 0 = illimité).
    pub max_upload_rate: u64,
    /// Transport uTP.
    pub utp: bool,
    /// DHT mainline BEP 5.
    pub dht: bool,
    /// Timeout de readiness DHT (s).
    pub dht_readiness_timeout: u64,
    /// UPnP.
    pub upnp: bool,
    /// NAT-PMP.
    pub natpmp: bool,
    /// Local service discovery.
    pub lsd: bool,
    /// Annonce à tous les tiers de trackers.
    pub announce_to_all_tiers: bool,
    /// Annonce à tous les trackers.
    pub announce_to_all_trackers: bool,
    /// Annonces HTTP simultanées max.
    pub max_concurrent_http_announces: u64,
    /// Re-hash après complétion.
    pub check_after_complete: bool,
    /// File d'attente : téléchargements actifs.
    pub active_downloads: i64,
    /// File d'attente : seeds actifs.
    pub active_seeds: i64,
    /// File d'attente : vérifications actives.
    pub active_checking: i64,
    /// Limite DHT active.
    pub active_dht_limit: i64,
    /// Limite trackers active.
    pub active_tracker_limit: i64,
    /// Limite LSD active.
    pub active_lsd_limit: i64,
    /// Limite globale active.
    pub active_limit: i64,
    /// Demande les réglages à l'ajout (émet `ask_add_download`).
    pub ask_download_settings: bool,
    /// Nettoie les .parts orphelins.
    pub clear_orphaned_parts: bool,
    /// Fichiers mappés mémoire.
    pub allow_mmap: bool,
    /// Défauts des nouveaux téléchargements.
    pub download_defaults: DownloadDefaultsConfig,
    /// Clés libtorrent additionnelles (`advanced_rate_limits`, …).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for LibtorrentConfig {
    fn default() -> Self {
        Self {
            socks_listen_ports: vec![0; 5],
            listen_interface: "0.0.0.0".into(),
            port: 0,
            listen_interface_v6: String::new(),
            port_v6: 0,
            proxy_type: 0,
            proxy_server: String::new(),
            proxy_auth: String::new(),
            max_connections_download: -1,
            max_download_rate: 0,
            max_upload_rate: 0,
            utp: true,
            dht: true,
            dht_readiness_timeout: 30,
            upnp: true,
            natpmp: true,
            lsd: true,
            announce_to_all_tiers: false,
            announce_to_all_trackers: false,
            max_concurrent_http_announces: 50,
            check_after_complete: false,
            active_downloads: 3,
            active_seeds: 5,
            active_checking: 1,
            active_dht_limit: 88,
            active_tracker_limit: 1600,
            active_lsd_limit: 60,
            active_limit: 500,
            ask_download_settings: false,
            clear_orphaned_parts: false,
            allow_mmap: true,
            download_defaults: DownloadDefaultsConfig::default(),
            extra: serde_json::Map::new(),
        }
    }
}

/// Section `tunnel_community`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TunnelCommunityConfig {
    /// Crée la TunnelCommunity et les lanes anonymes.
    pub enabled: bool,
    /// Circuits minimum maintenus (injecté dans `TunnelSettings`).
    pub min_circuits: u32,
    /// Circuits maximum.
    pub max_circuits: u32,
    /// Accepte d'être noeud de sortie (`exitnode_enabled` Tribler).
    pub exitnode_enabled: bool,
    /// Clés tunnel additionnelles — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for TunnelCommunityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_circuits: 3,
            max_circuits: 8,
            exitnode_enabled: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// Section `{enabled: bool}` générique (`database`, `dht_discovery`,
/// `recommender`, `rendezvous`, `torrent_checker`,
/// `content_discovery_community`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnabledSection {
    /// Composant actif.
    pub enabled: bool,
    /// Clés additionnelles — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl EnabledSection {
    /// Section activée (défaut de la plupart des composants Tribler).
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            extra: serde_json::Map::new(),
        }
    }
}

impl Default for EnabledSection {
    fn default() -> Self {
        Self::enabled()
    }
}

/// Section `rss`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RssConfig {
    /// Watchers RSS actifs.
    pub enabled: bool,
    /// Flux surveillés.
    pub urls: Vec<String>,
}

impl Default for RssConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            urls: Vec::new(),
        }
    }
}

/// Section `versioning`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VersioningConfig {
    /// Vérification de version.
    pub enabled: bool,
    /// Accepte les pré-versions.
    pub allow_pre: bool,
    /// Clés additionnelles.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for VersioningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_pre: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// Section `watch_folder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchFolderConfig {
    /// Surveillance active (défaut Python : false).
    pub enabled: bool,
    /// Répertoire surveillé.
    pub directory: String,
    /// Intervalle de scan (s).
    pub check_interval: f64,
}

impl Default for WatchFolderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: String::new(),
            check_interval: 10.0,
        }
    }
}

/// Arbre complet de `configuration.json` (équivalent `TriblerConfig`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Section `api`.
    pub api: ApiConfig,
    /// Section `ipv8`.
    pub ipv8: Ipv8FileConfig,
    /// Section `libtorrent`.
    pub libtorrent: LibtorrentConfig,
    /// Section `tunnel_community`.
    pub tunnel_community: TunnelCommunityConfig,
    /// Section `database`.
    pub database: EnabledSection,
    /// Section `dht_discovery`.
    pub dht_discovery: EnabledSection,
    /// Section `content_discovery_community`.
    pub content_discovery_community: EnabledSection,
    /// Section `recommender`.
    pub recommender: EnabledSection,
    /// Section `rendezvous`.
    pub rendezvous: EnabledSection,
    /// Section `rss`.
    pub rss: RssConfig,
    /// Section `torrent_checker`.
    pub torrent_checker: EnabledSection,
    /// Section `versioning`.
    pub versioning: VersioningConfig,
    /// Section `watch_folder`.
    pub watch_folder: WatchFolderConfig,
    /// Mode sans GUI.
    pub headless: bool,
    /// Démarrage minimisé (UI — inerte en daemon).
    pub start_minimized: bool,
    /// Stats IPv8 détaillées.
    pub statistics: bool,
    /// Base en mémoire (`:memory:`).
    pub memory_db: bool,
    /// Couleur de l'icône tray (UI — inerte en daemon).
    pub tray_icon_color: String,
    /// Réglages libres de l'UI (sparse).
    pub ui: Value,
    /// Sections/clés inconnues — préservées comme le dict Python.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            ipv8: Ipv8FileConfig::default(),
            libtorrent: LibtorrentConfig::default(),
            tunnel_community: TunnelCommunityConfig::default(),
            database: EnabledSection::enabled(),
            dht_discovery: EnabledSection::enabled(),
            content_discovery_community: EnabledSection::enabled(),
            recommender: EnabledSection::enabled(),
            rendezvous: EnabledSection::enabled(),
            rss: RssConfig::default(),
            torrent_checker: EnabledSection::enabled(),
            versioning: VersioningConfig::default(),
            watch_folder: WatchFolderConfig::default(),
            headless: false,
            start_minimized: false,
            statistics: false,
            memory_db: false,
            tray_icon_color: String::new(),
            ui: Value::Object(serde_json::Map::new()),
            extra: serde_json::Map::new(),
        }
    }
}

impl DaemonConfig {
    /// Charge `path` ; fichier absent ou corrompu → défauts
    /// (`Failed to load stored configuration. Falling back to
    /// defaults!` Python). La clé API est générée si absente.
    pub fn load(path: &Path) -> Self {
        Self::load_report(path).0
    }

    /// `load` + remontée de l'erreur de parse (`report_config_error`
    /// Python — le daemon peut la notifier sur le bus d'evenements).
    /// Retourne `(config, Some(erreur))` si le fichier existait mais
    /// n'etait pas un JSON valide.
    pub fn load_report(path: &Path) -> (Self, Option<String>) {
        let (mut cfg, error) = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %path.display(),
                        "configuration.json corrompu, repli sur les valeurs par défaut"
                    );
                    (Self::default(), Some(e.to_string()))
                }
            },
            Err(_) => (Self::default(), None),
        };
        cfg.ensure_api_key();
        (cfg, error)
    }

    /// Réécrit le fichier de configuration (`config.write()` Python).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// Génère `api.key` (32 hex) si vide — comme l'installeur Python.
    pub fn ensure_api_key(&mut self) {
        if self.api.key.is_empty() {
            let mut bytes = [0u8; 16];
            rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
            self.api.key = hex::encode(bytes);
        }
    }

    /// Clé API attendue (`None` = authentification inactive — le
    /// middleware Python autorise tout quand la clé est vide).
    pub fn api_key(&self) -> Option<&str> {
        (!self.api.key.is_empty()).then_some(self.api.key.as_str())
    }

    /// Merge récursif d'un patch JSON (`POST /api/settings`) : seuls
    /// les champs présents dans `patch` sont écrasés, l'arbre restant
    /// est conservé. Erreur si le résultat ne resérialise pas
    /// (types incohérents → 400 côté handler).
    pub fn merge(&mut self, patch: &Value) -> std::result::Result<(), serde_json::Error> {
        let mut merged = serde_json::to_value(&*self)?;
        deep_merge(&mut merged, patch);
        let next: Self = serde_json::from_value(merged)?;
        *self = next;
        Ok(())
    }

    /// Traduit l'arbre persisté en configuration de session coeur.
    /// `state_dir` vient du lanceur (le fichier vit dedans ; Python
    /// stocke `state_dir` dans la config, nous laissons le CLI décider).
    pub fn to_core_config(&self, state_dir: &Path) -> crate::CoreConfig {
        use tribler_network_policy::exit_policy as flags;

        let dd = &self.libtorrent.download_defaults;
        let downloads_dir = if dd.saveas.is_empty() {
            state_dir.join("downloads")
        } else {
            PathBuf::from(&dd.saveas)
        };

        let mut engine = tribler_bittorrent::EngineConfig {
            output_dir: downloads_dir.clone(),
            enable_dht: self.libtorrent.dht,
            disable_lsd: !self.libtorrent.lsd,
            listen_port: Some(self.libtorrent.port),
            // `max_*_rate` Python : 0 = illimite.
            max_upload_bps: (self.libtorrent.max_upload_rate > 0)
                .then_some(self.libtorrent.max_upload_rate),
            max_download_bps: (self.libtorrent.max_download_rate > 0)
                .then_some(self.libtorrent.max_download_rate),
            ..Default::default()
        };
        // proxy_type 2/3 = SOCKS5 (enum libtorrent). Le proxy guard
        // (loopback uniquement) reste appliqué au démarrage du moteur.
        if matches!(self.libtorrent.proxy_type, 2 | 3) && !self.libtorrent.proxy_server.is_empty() {
            engine.socks5_proxy = Some(format!("socks5://{}", self.libtorrent.proxy_server));
        }

        let listen = self
            .ipv8
            .interfaces
            .iter()
            .find(|i| i.interface == "UDPIPv4")
            .or_else(|| self.ipv8.interfaces.first());
        let mut peer_flags = flags::PEER_FLAG_RELAY;
        if self.tunnel_community.exitnode_enabled {
            peer_flags |=
                flags::PEER_FLAG_EXIT_BT | flags::PEER_FLAG_EXIT_IPV8 | flags::PEER_FLAG_EXIT_HTTP;
        }
        let ipv8 = crate::ipv8_stack::Ipv8Config {
            enabled: self.ipv8.enabled,
            listen_addr: listen.map_or_else(
                || format!("0.0.0.0:{}", crate::ipv8_stack::DEFAULT_IPV8_PORT),
                |i| format!("{}:{}", i.ip, i.port),
            ),
            bootstrap_peers: if self.ipv8.bootstrap.override_peers.is_empty() {
                crate::ipv8_stack::DEFAULT_BOOTSTRAP_PEERS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            } else {
                self.ipv8.bootstrap.override_peers.clone()
            },
            enable_anonymity: self.tunnel_community.enabled,
            peer_flags,
            tribler_tunnel_community: true,
            enable_dht: self.dht_discovery.enabled,
        };

        crate::CoreConfig {
            state_dir: state_dir.to_path_buf(),
            downloads_dir,
            db_filename: if self.memory_db {
                ":memory:".into()
            } else {
                "tribler.db".into()
            },
            ip_policy: tribler_network_policy::IpPolicy::strict(),
            watch_folder_dir: (self.watch_folder.enabled
                && !self.watch_folder.directory.is_empty())
            .then(|| PathBuf::from(&self.watch_folder.directory)),
            watch_folder_interval_ms: (self.watch_folder.check_interval.max(0.1) * 1000.0) as u64,
            rss_urls: if self.rss.enabled {
                self.rss.urls.clone()
            } else {
                Vec::new()
            },
            enable_torrent_checker: self.torrent_checker.enabled,
            ipv8,
            engine,
            download_defaults: crate::config::DownloadDefaults {
                anonymity_enabled: dd.anonymity_enabled,
                number_hops: dd.number_hops,
                safeseeding_enabled: dd.safeseeding_enabled,
                seeding_mode: dd.seeding_mode.clone(),
                seeding_ratio: dd.seeding_ratio,
                seeding_time: dd.seeding_time,
                auto_managed: dd.auto_managed,
                completed_dir: dd.completed_dir.clone(),
                trackers_file: dd.trackers_file.clone(),
                trackers_file_sync_url: dd.trackers_file_sync_url.clone(),
            },
            ..Default::default()
        }
    }

    /// Superpose les valeurs effectivement en cours (`CoreConfig`
    /// effective de la session, overrides à chaud compris) pour que
    /// `GET /api/settings` reflète l'état réel et pas seulement le
    /// fichier.
    pub fn apply_runtime_view(&mut self, core: &crate::CoreConfig) {
        self.libtorrent.download_defaults.saveas = core.engine.output_dir.display().to_string();
        self.libtorrent.dht = core.engine.enable_dht;
        self.libtorrent.lsd = !core.engine.disable_lsd;
        self.libtorrent.port = core.engine.listen_port.unwrap_or(0);
        self.libtorrent.proxy_type = if core.engine.socks5_proxy.is_some() {
            2
        } else {
            0
        };
        self.libtorrent.proxy_server = core.engine.socks5_proxy.clone().unwrap_or_default();
        self.libtorrent.max_upload_rate = core.engine.max_upload_bps.unwrap_or(0);
        self.libtorrent.max_download_rate = core.engine.max_download_bps.unwrap_or(0);
        self.rss.urls = core.rss_urls.clone();
        self.rss.enabled = !core.rss_urls.is_empty();
        self.watch_folder.enabled = core.watch_folder_dir.is_some();
        self.watch_folder.directory = core
            .watch_folder_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        self.torrent_checker.enabled = core.enable_torrent_checker;
        self.ipv8.enabled = core.ipv8.enabled;
        self.tunnel_community.enabled = core.ipv8.enable_anonymity;
        self.dht_discovery.enabled = core.ipv8.enable_dht;
    }
}
