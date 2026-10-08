// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration du moteur BitTorrent.
//!
//! Toute valeur de reglage vit ici (ou dans `onionbit-core` pour la
//! configuration utilisateur persistante) — jamais en dur dans la
//! logique.

use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::PathBuf;

/// Port TCP/uTP d'ecoute par defaut (0 = aleatoire attribue par l'OS).
pub const DEFAULT_LISTEN_PORT: u16 = 0;

/// Nombre maximal de pairs connectes par torrent par defaut.
pub const DEFAULT_PEER_LIMIT: usize = 64;

/// Nom de client annonce aux pairs/trackers (peer-id et User-Agent).
/// Aligné sur le format Azureus `-XX1234-` via `librqbit`.
pub const CLIENT_NAME: &str = "Tribler 8.3.0-rust";

/// `TX_BUF_SIZE_PER_VSOCK_INITIAL_DEFAULT` de librqbit-utp (le module
/// `constants` du vendored n'est pas public) — utilise uniquement
/// pour borner `initial <= max` quand un plafond TX est configure.
const UTP_TX_INITIAL_DEFAULT_BYTES: usize = 32 * 1024;

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
    /// `enable_upnp_port_forwarding` librqbit pour TCP + forwarder
    /// UDP local [`crate::upnp`] pour uTP — librqbit ne mappe que TCP).
    pub enable_upnp: bool,
    /// Redirection de port NAT-PMP / PCP (`libtorrent/natpmp` Tribler).
    pub enable_natpmp: bool,
    /// Limite de pairs par torrent (`libtorrent/
    /// max_connections_download` ; `None` = illimite, le `-1` Python).
    pub peer_limit: Option<usize>,
    /// Torrents en initialisation/verification simultanes
    /// (`libtorrent/active_checking` Tribler, rabattu sur
    /// `concurrent_init_limit` librqbit — le plus proche equivalent).
    pub concurrent_init_limit: Option<usize>,
    /// Proxy SOCKS5 pour les connexions sortantes vers les pairs —
    /// point d'integration futur des circuits `onionbit-tunnel`
    /// (anonymat). Doit etre une URL `socks5://host:port` ou
    /// `socks5h://` (resolution DNS cote proxy).
    ///
    /// SECURITE : cette valeur doit uniquement pointer vers le SOCKS5
    /// local expose par `onionbit-tunnel` (loopback), jamais vers un
    /// proxy distant non verifie — voir `onionbit-network-policy`.
    pub socks5_proxy: Option<String>,
    /// Activer le resume rapide (fastresume) : relit l'etat des pieces
    /// sans rehasher tout le contenu au demarrage. Sans effet sans
    /// `persistence_dir` — le fastresume rqbit exige un backend de
    /// persistance (le bitfield est ecrit dans `<ih>.bitv` apres le
    /// check initial).
    pub fastresume: bool,
    /// Echantillonnage de pieces a la restauration fastresume
    /// (`fastresume_sampled_check` librqbit — extension Tribler,
    /// pas d'equivalent Python) : quelques pieces par torrent sont
    /// relues au demarrage pour detecter des fichiers modifies entre
    /// deux runs. `false` = le `.bitv` est accepte tel quel —
    /// restauration quasi instantanee mais aucune detection de
    /// corruption deplacee/modifiee.
    pub fastresume_sampled_check: bool,
    /// Dossier de persistance de session (`SessionPersistenceConfig::
    /// Json` rqbit : `session.json` + `<ih>.bitv` + `<ih>.torrent`).
    /// Doit etre distinct par moteur — partager le dossier restaurerait
    /// les torrents anonymes sur le moteur en clair. `None` = pas de
    /// persistance (tests offline).
    pub persistence_dir: Option<PathBuf>,
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
    /// Socket uTP injectee pour les connexions pairs sortantes
    /// (`TunnelUdpSockets::utp` — lanes anonymes : le trafic part en
    /// cellules `data` du tunnel). Prioritaire sur la socket uTP
    /// d'ecoute cote librqbit.
    pub utp_socket: Option<std::sync::Arc<dyn librqbit::UtpConnector>>,
    /// Socket uTP injectee pour les connexions pairs ENTRANTES
    /// (`ListenerOptions::utp_socket` — lanes anonymes : le SYN uTP du
    /// downloader arrive en cellule `data` e2e, pas en UDP reel). Sans
    /// elle, les SYN entrants sont caches puis RST (pas d'accepteur) :
    /// le hidden seeding ne peut jamais recevoir de pair. Force un
    /// `ListenerOptions` `UtpOnly` meme si `listen_port` est `None`.
    pub utp_listen_socket: Option<std::sync::Arc<dyn librqbit::UtpAcceptor>>,
    /// Socket datagramme injectee pour la DHT (`DhtSessionConfig::
    /// socket` — lane anonyme : DHT routee dans le tunnel au lieu
    /// d'une socket UDP reelle).
    pub dht_socket: Option<std::sync::Arc<dyn librqbit::DatagramSocket>>,
    /// Adresses de bootstrap DHT (`DhtSessionConfig::bootstrap_addrs`)
    /// — `None` = les routeurs par defaut de librqbit.
    pub dht_bootstrap_addrs: Option<Vec<String>>,
    /// Port annonce en DHT/tracker (`ListenerOptions::announce_port`)
    /// — force l'annonce meme sur une ecoute loopback (tests, NAT
    /// connu). `None` = comportement librqbit (pas d'annonce en
    /// loopback).
    pub announce_port: Option<u16>,
    /// Socket datagramme injectee pour les trackers `udp://`
    /// (`SessionOptions::udp_tracker_socket` — anti-fuite : sans elle,
    /// les annonces UDP tracker partiraient en UDP clair hors tunnel).
    pub udp_tracker_socket: Option<std::sync::Arc<dyn librqbit::DatagramSocket>>,
    /// Plafond (s) du backoff exponentiel des re-lookups DHT
    /// `get_peers` sans progres — patch librqbit-dht vendored
    /// (`DhtSessionConfig::get_peers_backoff_cap`). `None` =
    /// intervalle fixe (comportement librqbit brut). Les lanes
    /// anonymes le configurent : un magnet sans swarm decroit vers
    /// une cadence plafonnee + jitter au lieu d'interroger la DHT a
    /// plein regime en permanence.
    pub dht_requery_backoff_cap_secs: Option<u64>,
    /// Budget global de traitement des requetes DHT entrantes
    /// (requetes/s, rafale bornee — l'excedent est ignore avant tout
    /// travail) — patch librqbit-dht vendored
    /// (`DhtSessionConfig::inbound_queries_per_second`). `None` =
    /// illimite (comportement librqbit brut). Defense en profondeur
    /// quand la socket DHT n'est pas en posture client-only : une
    /// requete entrante non sollicitee ne peut plus generer de
    /// reponse 1:1 ni de travail illimite.
    pub dht_inbound_queries_per_sec: Option<usize>,
    /// Plafond du buffer de reception uTP par connexion, en octets
    /// (extension Rust — pas de reglage libtorrent equivalent) : uTP
    /// bufferise en espace utilisateur — ce buffer est aussi la
    /// fenetre de reception annoncee au pair, donc le debit maximal
    /// d'une connexion (`fenetre / RTT`). `None` = defaut
    /// librqbit-utp (1 Mio). Poste memoire dominant identifie dans
    /// `docs/diagnostics/memoire_charge_reelle.md` — baisser borne la
    /// memoire RX pire cas par connexion, au prix du debit par
    /// connexion sur les lanes a RTT eleve.
    pub utp_rx_buf_size: Option<u64>,
    /// Plafond du buffer d'emission uTP par connexion, en octets
    /// (extension Rust) : borne les donnees non acquittees stockees
    /// (memoire TX pire cas par connexion). `None` = defaut
    /// librqbit-utp (croissance 32 Kio -> 1 Mio). Le buffer demarre
    /// au minimum du plafond et de l'initial vendored (32 Kio).
    pub utp_tx_buf_max: Option<u64>,
    /// Borne du semaphore d'appels bloquants de librqbit
    /// (`SessionOptions::runtime_worker_threads`) : les I/O disque
    /// synchrones (hash de pieces, sparse marking, `ensure_file_length`)
    /// passent par `tokio::task::block_in_place` qui consomme des
    /// threads workers de l'executor. Sans borne < nb de workers,
    /// la restauration de gros torrents multi-fichiers peut figer
    /// l'executor entier (API muette, tunnels geles). Defaut : nb de
    /// coeurs — les workers Tokio doivent etre plus nombreux (cf.
    /// `worker_threads` dans `onionbit-daemon`).
    pub runtime_worker_threads: Option<usize>,
    /// Zone privee (ADR-0018) : configuration d'opacite `.bitv` —
    /// enrobage `OpaqueBitVFactory` injecte dans
    /// `SessionOptions::bitv_factory_wrapper` (les `.bitv` prives sont
    /// ecrits sous des noms HMAC au lieu de `<infohash>.bitv` clair) +
    /// set partage + nettoyage a la suppression.
    pub opaque_bitv: Option<crate::bitv_opaque::OpaqueBitV>,
}

/// Signature de la closure d'enrobage `BitVFactory` injectee dans
/// `SessionOptions::bitv_factory_wrapper` (patch vendored, ADR-0018).
pub type BitVWrapperFn = std::sync::Arc<
    dyn Fn(std::sync::Arc<dyn librqbit::BitVFactory>) -> std::sync::Arc<dyn librqbit::BitVFactory>
        + Send
        + Sync,
>;

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
            enable_natpmp: true,
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            concurrent_init_limit: None,
            socks5_proxy: None,
            fastresume: true,
            fastresume_sampled_check: true,
            persistence_dir: None,
            utp_only: false,
            max_upload_bps: None,
            max_download_bps: None,
            allow_mmap: true,
            clear_orphaned_parts: false,
            dht_readiness_timeout_secs: 0,
            utp_socket: None,
            utp_listen_socket: None,
            dht_socket: None,
            dht_bootstrap_addrs: None,
            announce_port: None,
            udp_tracker_socket: None,
            dht_requery_backoff_cap_secs: None,
            dht_inbound_queries_per_sec: None,
            utp_rx_buf_size: None,
            utp_tx_buf_max: None,
            runtime_worker_threads: default_runtime_worker_threads(),
            opaque_bitv: None,
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
            enable_natpmp: false,
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            concurrent_init_limit: None,
            socks5_proxy: None,
            fastresume: false,
            fastresume_sampled_check: true,
            persistence_dir: None,
            utp_only: false,
            max_upload_bps: None,
            max_download_bps: None,
            allow_mmap: false,
            clear_orphaned_parts: false,
            dht_readiness_timeout_secs: 0,
            utp_socket: None,
            utp_listen_socket: None,
            dht_socket: None,
            dht_bootstrap_addrs: None,
            announce_port: None,
            udp_tracker_socket: None,
            dht_requery_backoff_cap_secs: None,
            dht_inbound_queries_per_sec: None,
            utp_rx_buf_size: None,
            utp_tx_buf_max: None,
            runtime_worker_threads: default_runtime_worker_threads(),
            opaque_bitv: None,
        }
    }

    /// Options `librqbit_utp::SocketOpts` deduites de la config :
    /// plafonds de buffers par connexion. `SocketOpts::default()`
    /// quand rien n'est configure — strictement le comportement
    /// vendored (RX 1 Mio, TX 32 Kio -> 1 Mio par stream). Le meme
    /// objet est applique aux sockets uTP reelles de la session
    /// (`ListenerOptions::utp_opts` — ecoute + connexions sortantes,
    /// qui retombent sur la socket d'ecoute) et aux sockets uTP
    /// tunnelsees des lanes anonymes (`TunnelUdpSockets`).
    pub fn utp_socket_opts(&self) -> librqbit_utp::SocketOpts {
        let mut opts = librqbit_utp::SocketOpts::default();
        if let Some(rx) = self
            .utp_rx_buf_size
            .and_then(|v| usize::try_from(v).ok())
            .and_then(NonZeroUsize::new)
        {
            opts.vsock_rx_bufsize_bytes = Some(rx);
        }
        if let Some(max) = self
            .utp_tx_buf_max
            .and_then(|v| usize::try_from(v).ok())
            .and_then(NonZeroUsize::new)
        {
            opts.vsock_tx_bufsize_bytes_max = Some(max);
            // Coherence : le ring buffer TX demarre a `initial`
            // (32 Kio vendored) puis croit vers `max` — un plafond
            // sous l'initial demarrerait deja au-dessus de lui.
            opts.vsock_tx_bufsize_bytes_initial =
                NonZeroUsize::new(max.get().min(UTP_TX_INITIAL_DEFAULT_BYTES));
        }
        opts
    }

    /// Traduit la config en `librqbit::SessionOptions`.
    pub(crate) fn to_session_options(&self) -> librqbit::SessionOptions {
        let connect = if self.socks5_proxy.is_some() || self.utp_only || self.utp_socket.is_some() {
            Some(librqbit::ConnectionOptions {
                proxy_url: self.socks5_proxy.clone(),
                enable_tcp: !self.utp_only,
                utp_socket: self.utp_socket.clone(),
                ..Default::default()
            })
        } else {
            None
        };

        let dht = if self.enable_dht {
            Some(librqbit::DhtSessionConfig {
                persistence: None,
                socket: self.dht_socket.clone(),
                bootstrap_addrs: self.dht_bootstrap_addrs.clone(),
                get_peers_backoff_cap: self
                    .dht_requery_backoff_cap_secs
                    .map(std::time::Duration::from_secs),
                inbound_queries_per_second: self.dht_inbound_queries_per_sec,
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
            fastresume_sampled_check: self.fastresume_sampled_check,
            persistence: if self.fastresume {
                self.persistence_dir.clone().map(|folder| {
                    librqbit::SessionPersistenceConfig::Json {
                        folder: Some(folder),
                        // Tribler : la relecture de `session.json` au
                        // demarrage est desactivee — le checkpoint
                        // tribler.db est l'unique autorite qui reajoute
                        // les torrents (sinon les deux restaurations
                        // tournent en concurrence et figent des
                        // torrents en `Initializing`). Les `.bitv`
                        // fastresume restent ecrits et relus.
                        restore: false,
                    }
                })
            } else {
                None
            },
            peer_limit: self.peer_limit,
            concurrent_init_limit: self.concurrent_init_limit,
            default_storage_factory: (self.allow_mmap && !self.utp_only).then(|| {
                use librqbit::storage::StorageFactoryExt;
                librqbit::storage::examples::mmap::MmapFilesystemStorageFactory {}.boxed()
            }),
            client_name_and_version: Some(CLIENT_NAME.to_string()),
            listen: if let Some(acceptor) = self.utp_listen_socket.clone() {
                // Lane anonyme : ecoute uTP sur le transport tunnelse
                // (pas de bind UDP reel — adresse loopback factice, pas
                // de port annonce, pas d'UPnP).
                Some(librqbit::ListenerOptions {
                    mode: librqbit::ListenerMode::UtpOnly,
                    listen_addr: SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0),
                    enable_upnp_port_forwarding: false,
                    announce_port: None,
                    utp_socket: Some(acceptor),
                    ..Default::default()
                })
            } else {
                self.listen_addr_v6
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
                        announce_port: self.announce_port,
                        // Plafonds de buffers uTP par connexion —
                        // `Some(defauts)` valide exactement comme
                        // `None` quand rien n'est configure.
                        utp_opts: Some(self.utp_socket_opts()),
                        ..Default::default()
                    })
            },
            connect,
            udp_tracker_socket: self.udp_tracker_socket.clone(),
            runtime_worker_threads: self.runtime_worker_threads,
            bitv_factory_wrapper: self.opaque_bitv.as_ref().map(|o| o.wrapper()),
            ..Default::default()
        }
    }
}

/// Borne par defaut du semaphore d'I/O bloquantes de librqbit :
/// nombre de coeurs physiques/logiques visibles par le process.
/// Volontairement inferieur au nombre de workers Tokio (2 x coeurs
/// dans `onionbit-daemon`) pour qu'il reste toujours des threads
/// libres quand tous les slots d'I/O sont occupes.
fn default_runtime_worker_threads() -> Option<usize> {
    Some(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(2),
    )
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use super::*;

    /// Accepteur uTP factice : jamais appele (test de configuration
    /// uniquement — le chemin `accept()` est couvert par
    /// `tests/utp_acceptor.rs`).
    #[derive(Debug)]
    struct NoAcceptor;

    impl librqbit::UtpAcceptor for NoAcceptor {
        fn accept(
            self: Arc<Self>,
        ) -> Pin<
            Box<
                dyn Future<Output = librqbit_utp::Result<librqbit_utp::UtpStream>>
                    + Send
                    + Sync
                    + 'static,
            >,
        > {
            Box::pin(async { unreachable!("jamais appele") })
        }
    }

    /// Anti-fuite : une lane anonyme (`utp_listen_socket` injectee,
    /// `listen_port` absent) doit produire un `ListenerOptions`
    /// `UtpOnly` sur l'accepteur injecte, sans port annonce ni UPnP
    /// — le seeder cache n'expose aucune socket UDP reelle.
    #[test]
    fn lane_anonyme_ecoute_utp_sans_fuite() {
        let mut cfg = EngineConfig::offline(PathBuf::from("dl"));
        cfg.utp_only = true;
        cfg.utp_listen_socket = Some(Arc::new(NoAcceptor));

        let opts = cfg.to_session_options();
        let listen = opts.listen.expect("lane anonyme sans listener");
        assert!(
            matches!(listen.mode, librqbit::ListenerMode::UtpOnly),
            "mode attendu UtpOnly : {:?}",
            listen.mode
        );
        assert!(listen.utp_socket.is_some(), "accepteur injecte absent");
        assert!(listen.announce_port.is_none(), "port annonce en DHT");
        assert!(
            !listen.enable_upnp_port_forwarding,
            "UPnP actif sur lane anonyme"
        );
        assert!(
            listen.listen_addr.ip().is_loopback(),
            "bind non loopback : {:?}",
            listen.listen_addr
        );
    }

    /// Plafonds uTP (`utp_rx_buf_size`/`utp_tx_buf_max`) : les
    /// `SocketOpts` produits portent les valeurs — et le buffer TX
    /// initial reste borne au plafond quand celui-ci descend sous
    /// l'initial vendored (32 Kio). Rien configure = strictement les
    /// defauts vendored.
    #[test]
    fn utp_socket_opts_plafonds() {
        // Defauts : aucun champ pose -> SocketOpts brut vide (le
        // `validate()` librqbit-utp appliquera ses defauts).
        let opts = EngineConfig::default().utp_socket_opts();
        assert!(opts.vsock_rx_bufsize_bytes.is_none());
        assert!(opts.vsock_tx_bufsize_bytes_max.is_none());
        assert!(opts.vsock_tx_bufsize_bytes_initial.is_none());

        let cfg = EngineConfig {
            utp_rx_buf_size: Some(256 * 1024),
            utp_tx_buf_max: Some(64 * 1024),
            ..Default::default()
        };
        let opts = cfg.utp_socket_opts();
        assert_eq!(
            opts.vsock_rx_bufsize_bytes.map(NonZeroUsize::get),
            Some(256 * 1024)
        );
        assert_eq!(
            opts.vsock_tx_bufsize_bytes_max.map(NonZeroUsize::get),
            Some(64 * 1024)
        );
        // Plafond > 32 Kio : l'initial reste la valeur vendored.
        assert_eq!(
            opts.vsock_tx_bufsize_bytes_initial.map(NonZeroUsize::get),
            Some(32 * 1024)
        );

        // Plafond < 32 Kio : l'initial est ramene au plafond.
        let cfg = EngineConfig {
            utp_tx_buf_max: Some(8 * 1024),
            ..Default::default()
        };
        let opts = cfg.utp_socket_opts();
        assert_eq!(
            opts.vsock_tx_bufsize_bytes_initial.map(NonZeroUsize::get),
            Some(8 * 1024)
        );
    }

    /// Les plafonds uTP arrivent dans `ListenerOptions::utp_opts` de
    /// la session non injectee — la meme socket sert l'ecoute et les
    /// connexions sortantes (`StreamConnector` retombe sur la socket
    /// d'ecoute quand aucune n'est injectee).
    #[test]
    fn utp_opts_propagees_au_listener() {
        let cfg = EngineConfig {
            utp_rx_buf_size: Some(256 * 1024),
            utp_tx_buf_max: Some(128 * 1024),
            ..Default::default()
        };
        let listen = cfg
            .to_session_options()
            .listen
            .expect("listener attendu (listen_port par defaut)");
        let opts = listen.utp_opts.expect("utp_opts absents");
        assert_eq!(
            opts.vsock_rx_bufsize_bytes.map(NonZeroUsize::get),
            Some(256 * 1024)
        );
        assert_eq!(
            opts.vsock_tx_bufsize_bytes_max.map(NonZeroUsize::get),
            Some(128 * 1024)
        );
    }
}
