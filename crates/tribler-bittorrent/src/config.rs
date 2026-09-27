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
    /// Limite de pairs par torrent.
    pub peer_limit: usize,
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
            peer_limit: DEFAULT_PEER_LIMIT,
            socks5_proxy: None,
            fastresume: true,
            utp_only: false,
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
            peer_limit: DEFAULT_PEER_LIMIT,
            socks5_proxy: None,
            fastresume: false,
            utp_only: false,
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

        librqbit::SessionOptions {
            dht: self.enable_dht.then(librqbit::DhtSessionConfig::default),
            disable_trackers: self.disable_trackers,
            disable_local_service_discovery: self.disable_lsd,
            ipv4_only: self.ipv4_only,
            fastresume: self.fastresume,
            peer_limit: Some(self.peer_limit),
            client_name_and_version: Some(CLIENT_NAME.to_string()),
            listen: self.listen_port.map(|port| librqbit::ListenerOptions {
                mode: if self.utp_only {
                    librqbit::ListenerMode::UtpOnly
                } else {
                    librqbit::ListenerMode::TcpAndUtp
                },
                listen_addr: SocketAddr::from(([0, 0, 0, 0], port)),
                enable_upnp_port_forwarding: false,
                ..Default::default()
            }),
            connect,
            ..Default::default()
        }
    }
}
