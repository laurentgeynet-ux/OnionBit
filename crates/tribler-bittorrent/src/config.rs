//! Configuration du moteur BitTorrent.
//!
//! Toute valeur de reglage vit ici (ou dans `tribler-core` pour la
//! configuration utilisateur persistante) — jamais en dur dans la
//! logique.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Port TCP/uTP d'ecoute par defaut (0 = aleatoire attribue par l'OS).
pub const DEFAULT_LISTEN_PORT: u16 = 0;

/// Nombre maximal de pairs connectes par torrent par defaut.
pub const DEFAULT_PEER_LIMIT: usize = 64;

/// Nom de client annonce aux pairs/trackers (peer-id et User-Agent).
/// Aligné sur le format Azureus `-XX1234-` via `librqbit`.
pub const CLIENT_NAME: &str = "Tribler 8.3.0-rust";

/// Configuration d'une session BitTorrent.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Repertoire de telechargement par defaut.
    pub output_dir: PathBuf,
    /// Activer la DHT mainline (BEP 5). Desactiver pour les tests
    /// offline ou les telechargements purement prives.
    pub enable_dht: bool,
    /// Desactiver toute communication avec les trackers.
    pub disable_trackers: bool,
    /// Desactiver la decouverte locale de pairs (LSD multicast).
    pub disable_lsd: bool,
    /// Forcer IPv4 uniquement.
    pub ipv4_only: bool,
    /// Port d'ecoute entrant (TCP + uTP). `None` desactive l'ecoute
    /// entrante (utile en test offline ou mode "sans seed entrant").
    pub listen_port: Option<u16>,
    /// Adresse IPv4 d'ecoute (`libtorrent/listen_interface` Tribler).
    /// Ignoree quand `listen_addr_v6` est `Some`.
    pub listen_ip: std::net::IpAddr,
    /// Ecoute IPv6 (`libtorrent/listen_interface_v6` + `port_v6`
    /// Tribler, `""` = desactivee). `Some` remplace le bind IPv4 : la
    /// socket `[v6]` est dual-stack et accepte aussi le v4 — librqbit
    /// n'ouvre qu'un seul socket d'ecoute.
    pub listen_addr_v6: Option<SocketAddr>,
    /// Transport uTP (`libtorrent/utp` Tribler) — `false` = TCP seul.
    /// Sans effet quand `utp_only` (lanes anonymes : toujours uTP).
    pub enable_utp: bool,
    /// Redirection de port UPnP (`libtorrent/upnp` Tribler ->
    /// `enable_upnp_port_forwarding` librqbit).
    pub enable_upnp: bool,
    /// Limite de pairs par torrent (`libtorrent/
    /// max_connections_download` ; `None` = illimite, le `-1` Python).
    pub peer_limit: Option<usize>,
    /// Torrents en initialisation/verification simultanes
    /// (`libtorrent/active_checking` Tribler, rabattu sur
    /// `concurrent_init_limit` librqbit — le plus proche equivalent).
    pub concurrent_init_limit: Option<usize>,
    /// Proxy SOCKS5 pour les connexions sortantes vers les pairs —
    /// point d'integration futur des circuits `tribler-tunnel`
    /// (anonymat). Doit etre une URL `socks5://host:port` ou
    /// `socks5h://` (resolution DNS cote proxy).
    ///
    /// SECURITE : cette valeur doit uniquement pointer vers le SOCKS5
    /// local expose par `tribler-tunnel` (loopback), jamais vers un
    /// proxy distant non verifie — voir `tribler-network-policy`.
    pub socks5_proxy: Option<String>,
    /// Activer le resume rapide (fastresume) : relit l'etat des pieces
    /// sans rehasher tout le contenu au demarrage.
    pub fastresume: bool,
    /// Mode uTP uniquement (hidden seeding : les tunnels Tribler ne
    /// transportent que de l'UDP — les connexions TCP sortantes et
    /// l'ecoute TCP sont desactivees).
    pub utp_only: bool,
    /// Limite globale d'upload en octets/s (`None` = illimite ;
    /// `libtorrent/max_upload_rate` Tribler). Modifiable a chaud via
    /// `BtEngine::set_ratelimits`.
    pub max_upload_bps: Option<u64>,
    /// Limite globale de download en octets/s (`libtorrent/
    /// max_download_rate` Tribler).
    pub max_download_bps: Option<u64>,
    /// Stockage mmap (`libtorrent/allow_mmap` Tribler ->
    /// `default_storage_factory` librqbit `MmapFilesystemStorageFactory`).
    /// Sans effet quand `utp_only` : Tribler restreint le mmap a la
    /// session par defaut (`enable_mmap and hops < 0` Python).
    pub allow_mmap: bool,
    /// Purge des fichiers `.parts` orphelins du repertoire de sortie a
    /// l'arret (`libtorrent/clear_orphaned_parts` Tribler ->
    /// `rm_orphaned_files_and_subfolders` : librqbit n'ecrit pas de
    /// `.parts` lui-meme, le nettoyage porte sur les restes laisses
    /// par d'autres clients libtorrent dans le meme dossier).
    pub clear_orphaned_parts: bool,
    /// Attente de la DHT au demarrage en secondes (`libtorrent/
    /// dht_readiness_timeout` Tribler) : la session attend que la
    /// table de routage DHT se peuple avant d'etre prete. `0` = pas
    /// d'attente ; sans effet quand `enable_dht` est faux.
    pub dht_readiness_timeout_secs: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("downloads"),
            enable_dht: true,
            disable_trackers: false,
            disable_lsd: false,
            ipv4_only: false,
            listen_port: Some(DEFAULT_LISTEN_PORT),
            listen_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            listen_addr_v6: None,
            enable_utp: true,
            enable_upnp: true,
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            concurrent_init_limit: None,
            socks5_proxy: None,
            fastresume: true,
            utp_only: false,
            max_upload_bps: None,
            max_download_bps: None,
            allow_mmap: true,
            clear_orphaned_parts: false,
            dht_readiness_timeout_secs: 0,
        }
    }
}

impl EngineConfig {
    /// Configuration isolee pour les tests : aucun trafic reseau sortant
    /// (pas de DHT, pas de trackers, pas d'ecoute, pas de LSD).
    pub fn offline(output_dir: PathBuf) -> Self {
        Self {
            output_dir,
            enable_dht: false,
            disable_trackers: true,
            disable_lsd: true,
            ipv4_only: true,
            listen_port: None,
            listen_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            listen_addr_v6: None,
            enable_utp: true,
            enable_upnp: false,
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            concurrent_init_limit: None,
            socks5_proxy: None,
            fastresume: false,
            utp_only: false,
            max_upload_bps: None,
            max_download_bps: None,
            allow_mmap: false,
            clear_orphaned_parts: false,
            dht_readiness_timeout_secs: 0,
        }
    }

    /// Traduit la config en `librqbit::SessionOptions`.
    pub(crate) fn to_session_options(&self) -> librqbit::SessionOptions {
        let connect = if self.socks5_proxy.is_some() || self.utp_only {
            Some(librqbit::ConnectionOptions {
                proxy_url: self.socks5_proxy.clone(),
                enable_tcp: !self.utp_only,
                ..Default::default()
            })
        } else {
            None
        };

        let dht = if self.enable_dht {
            Some(librqbit::DhtSessionConfig {
                persistence: None,
                ..Default::default()
            })
        } else {
            None
        };

        librqbit::SessionOptions {
            dht,
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: self
                    .max_upload_bps
                    .and_then(|v| u32::try_from(v).ok())
                    .and_then(std::num::NonZeroU32::new),
                download_bps: self
                    .max_download_bps
                    .and_then(|v| u32::try_from(v).ok())
                    .and_then(std::num::NonZeroU32::new),
            },
            disable_trackers: self.disable_trackers,
            disable_local_service_discovery: self.disable_lsd,
            ipv4_only: self.ipv4_only,
            fastresume: self.fastresume,
            peer_limit: self.peer_limit,
            concurrent_init_limit: self.concurrent_init_limit,
            default_storage_factory: (self.allow_mmap && !self.utp_only).then(|| {
                use librqbit::storage::StorageFactoryExt;
                librqbit::storage::examples::mmap::MmapFilesystemStorageFactory {}.boxed()
            }),
            client_name_and_version: Some(CLIENT_NAME.to_string()),
            listen: self
                .listen_addr_v6
                .or_else(|| {
                    self.listen_port
                        .map(|port| SocketAddr::new(self.listen_ip, port))
                })
                .map(|addr| librqbit::ListenerOptions {
                    mode: if self.utp_only {
                        librqbit::ListenerMode::UtpOnly
                    } else if self.enable_utp {
                        librqbit::ListenerMode::TcpAndUtp
                    } else {
                        librqbit::ListenerMode::TcpOnly
                    },
                    listen_addr: addr,
                    enable_upnp_port_forwarding: self.enable_upnp,
                    ..Default::default()
                }),
            connect,
            ..Default::default()
        }
    }
}
