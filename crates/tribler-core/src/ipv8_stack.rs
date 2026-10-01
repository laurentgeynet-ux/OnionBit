//! Stack IPv8 optionnelle de la session (etape 15).
//!
//! Equivalent des composants `IPv8`/`DiscoveryCommunity`/
//! `ContentDiscoveryCommunity`/`TunnelCommunity` de
//! `tribler.core.components` : quand `CoreConfig.ipv8.enabled`,
//! `CoreSession` cree un endpoint UDP IPv8, la discovery, la community
//! de decouverte de contenu et — si `enable_anonymity` — la
//! `TunnelCommunity` avec des proxys SOCKS5 par nombre de sauts
//! (`1..=MAX_ANON_HOPS`) desservant chacun un `BtEngine` dedie
//! (uTP-only + SOCKS5, comme les sessions libtorrent anonymes de
//! Tribler).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tribler_bittorrent::{BtEngine, EngineConfig};
use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_db::Database;
use tribler_ipv8::content_discovery::{
    ContentDiscoveryCommunity, ContentProvider, HealthInfo, HEALTH_REQUEST_POPULAR,
};
use tribler_ipv8::dht::DhtCommunity;
use tribler_ipv8::discovery::DiscoveryCommunity;
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::hidden_services::lookup_info_hash;
use tribler_tunnel::routing::{circuit_id_to_ip, CIRCUIT_ID_PORT};
use tribler_tunnel::socks5::Socks5Server;
use tribler_tunnel::tunnel_udp_socket::TunnelUdpSocket;
use tribler_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID;

use crate::error::{CoreError, Result};

/// Fichier de cle IPv8 persiste dans `state_dir` (`ec.pem`-equivalent,
/// format binaire `LibNaCLSK:`).
const IPV8_KEY_FILE: &str = "ipv8_keypair.bin";

/// Nombre de sauts anonymes maximum (`anon_hops` Python : 1..=3).
pub const MAX_ANON_HOPS: usize = 3;

/// Port d'ecoute UDP IPv8 par defaut (fidele a `pyipv8` : 8090).
pub const DEFAULT_IPV8_PORT: u16 = 8090;

/// Noeuds d'amorcage IPv8 officiels (Dispersy / TU Delft),
/// fideles a `DISPERSY_BOOTSTRAPPER` de `pyipv8.ipv8.configuration`.
pub const DEFAULT_BOOTSTRAP_PEERS: &[&str] = &[
    "130.161.119.206:6421",
    "130.161.119.206:6422",
    "131.180.27.155:6423",
    "131.180.27.156:6424",
    "131.180.27.161:6427",
    "131.180.27.161:6521",
    "131.180.27.161:6522",
    "131.180.27.162:6523",
    "131.180.27.162:6524",
    "130.161.119.215:6525",
    "130.161.119.215:6526",
    "130.161.119.201:6527",
    "130.161.119.201:6528",
    "dispersy1.tribler.org:6421",
    "dispersy1.st.tudelft.nl:6421",
    "dispersy2.tribler.org:6422",
    "dispersy2.st.tudelft.nl:6422",
    "dispersy3.tribler.org:6423",
    "dispersy3.st.tudelft.nl:6423",
    "dispersy4.tribler.org:6424",
];

/// Configuration de la stack IPv8 de session.
///
/// `enabled = false` (defaut) : session sans overlay — identique a
/// `ipv8.enabled = false` cote Tribler.
#[derive(Debug, Clone)]
pub struct Ipv8Config {
    /// Active la stack IPv8 (endpoint UDP + discovery + content
    /// discovery + tunnel si `enable_anonymity`).
    pub enabled: bool,
    /// Adresse d'ecoute UDP de l'endpoint (`"0.0.0.0:0"` = port
    /// ephemere, tests ; Tribler utilise un port configure).
    pub listen_addr: String,
    /// Pairs d'amorcage `"host:port"` (bootstrap discovery).
    pub bootstrap_peers: Vec<String>,
    /// Cree la `TunnelCommunity` + les proxys SOCKS5 anonymes
    /// (requis pour `anon_hops > 0` sur les telechargements).
    pub enable_anonymity: bool,
    /// Flags de service annonces aux pairs tunnel
    /// (`PEER_FLAG_*` — `RELAY`/`EXIT_BT`/...).
    pub peer_flags: i32,
    /// Utilise le `community_id` de `TriblerTunnelCommunity` au lieu
    /// du `pyipv8` (interop avec les clients Tribler installes).
    pub tribler_tunnel_community: bool,
    /// Cree la `DHTDiscoveryCommunity` (`dht_discovery/enabled`
    /// Python — precondition de `DHTDiscoveryComponent`).
    pub enable_dht: bool,
    /// `walker_interval` Python (s) — cadence des strategies de
    /// decouverte ; sert de reference a `DriftMeasurementStrategy`
    /// (`/api/ipv8/asyncio/drift`).
    pub walker_interval: f64,
    /// `TunnelSettings.min_circuits` Python : nombre de circuits
    /// `READY` (ou en cours) maintenus par lane anonyme. Redondance
    /// necessaire — un unique circuit dont le dernier saut annonce
    /// `EXIT_HTTP` sans le servir reellement (pair instable ou
    /// mensonger du reseau public) bloque la lane pour toujours
    /// (`aucun circuit HTTP pret`/timeouts) puisque le watchdog ne
    /// retente jamais tant qu'un circuit `READY` existe.
    pub min_circuits: u32,
    /// `TunnelSettings.max_circuits` Python : borne haute de la cible
    /// du watchdog (`circuits_needed[hops] = clamp(downloads actifs,
    /// min_circuits, max_circuits)` — `monitor_downloads` de
    /// `TriblerTunnelCommunity`).
    pub max_circuits: u32,
    /// `libtorrent/socks_listen_ports` Python : ports des proxys
    /// SOCKS5 anonymes, index `[hops-1]` (0 = port ephemere attribue
    /// par l'OS, defaut Tribler `[0]*5`).
    pub socks_listen_ports: Vec<u16>,
    /// `content_discovery_community/enabled` Python : cree la
    /// `ContentDiscoveryCommunity` (recherche distante, select).
    pub enable_content_discovery: bool,
    /// Extension Rust : point d'introduction impose pour les circuits
    /// `IP_SEEDER` (`required_ip` de `create_introduction_point`
    /// pyipv8). `None` = selection automatique (`select_exit`).
    pub intro_point_peer: Option<UdpAddress>,
    /// Extension Rust : sortie imposee pour les circuits `DATA`
    /// (`required_exit` de `create_circuit` pyipv8). `None` =
    /// selection automatique `EXIT_BT`.
    pub data_exit_peer: Option<UdpAddress>,
    /// Extension Rust (bancs loopback) : estimation WAN initiale.
    /// Sur un mesh 100 % loopback, `destination_address` des
    /// intro-responses est toujours en sous-reseau LAN →
    /// `my_estimated_wan` resterait indetermine et le DHT muet
    /// (`on_node_discovered` refuse tout noeud sans WAN connu —
    /// meme comportement pyipv8). `None` = apprentissage normal.
    pub estimated_wan: Option<UdpAddress>,
    /// `ipv8/interfaces[UDPIPv6]` Python (`ip:port`, ex. `"[::]:8091"`)
    /// — socket UDP secondaire partageant les memes communities
    /// (`DispatcherEndpoint` pyipv8). `None` = IPv4 seul.
    pub listen_addr_v6: Option<String>,
    /// Extension Rust : nombre max de pairs verifies persistes dans
    /// `ipv8_peers` (recharges au demarrage pour un bootstrap rapide —
    /// pyipv8 ne persiste pas son annuaire).
    pub peer_cache_max: usize,
    /// Age max d'un pair cache (s) — au-dela il est expire au chargement
    /// puis supprime par le prune periodique.
    pub peer_cache_max_age_secs: u64,
    /// Intervalle du snapshot `Network` -> `ipv8_peers` (s).
    pub peer_persist_interval_secs: u64,
}

impl Ipv8Config {
    /// Configuration pour une session connectee au reseau reel :
    /// IPv8 actif sur le port configure (ou 8090 par defaut),
    /// noeuds d'amorcage officiels, tunnels actifs avec l'identifiant
    /// de communaute Tribler officiel.
    pub fn production() -> Self {
        Self {
            enabled: true,
            listen_addr: format!("0.0.0.0:{DEFAULT_IPV8_PORT}"),
            bootstrap_peers: DEFAULT_BOOTSTRAP_PEERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            enable_anonymity: true,
            peer_flags: tribler_network_policy::exit_policy::PEER_FLAG_RELAY,
            tribler_tunnel_community: true,
            enable_dht: true,
            walker_interval: DEFAULT_WALKER_INTERVAL,
            min_circuits: DEFAULT_MIN_CIRCUITS,
            max_circuits: DEFAULT_MAX_CIRCUITS,
            socks_listen_ports: vec![0; MAX_ANON_HOPS],
            enable_content_discovery: true,
            intro_point_peer: None,
            data_exit_peer: None,
            estimated_wan: None,
            listen_addr_v6: Some(format!("[::]:{}", DEFAULT_IPV8_PORT + 1)),
            peer_cache_max: DEFAULT_PEER_CACHE_MAX,
            peer_cache_max_age_secs: DEFAULT_PEER_CACHE_MAX_AGE_SECS,
            peer_persist_interval_secs: DEFAULT_PEER_PERSIST_INTERVAL_SECS,
        }
    }
}

impl Default for Ipv8Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: "0.0.0.0:0".into(),
            bootstrap_peers: Vec::new(),
            enable_anonymity: false,
            peer_flags: tribler_network_policy::exit_policy::PEER_FLAG_RELAY,
            tribler_tunnel_community: false,
            // `dht_discovery.enabled` vaut `true` par defaut dans
            // `tribler_config` — la community n'est creee que si
            // `enabled` est vrai de toute facon.
            enable_dht: true,
            walker_interval: DEFAULT_WALKER_INTERVAL,
            min_circuits: DEFAULT_MIN_CIRCUITS,
            max_circuits: DEFAULT_MAX_CIRCUITS,
            socks_listen_ports: vec![0; MAX_ANON_HOPS],
            enable_content_discovery: true,
            intro_point_peer: None,
            data_exit_peer: None,
            estimated_wan: None,
            listen_addr_v6: None,
            peer_cache_max: DEFAULT_PEER_CACHE_MAX,
            peer_cache_max_age_secs: DEFAULT_PEER_CACHE_MAX_AGE_SECS,
            peer_persist_interval_secs: DEFAULT_PEER_PERSIST_INTERVAL_SECS,
        }
    }
}

/// Lane anonyme : proxy SOCKS5 + moteur dedies a un nombre de sauts
/// (equivalent des sessions libtorrent `hops=1..3` de Tribler).
struct AnonLane {
    /// Port du proxy SOCKS5 (loopback) de cette lane.
    pub socks_addr: SocketAddr,
    /// Garde le serveur SOCKS5 en vie.
    #[allow(dead_code)]
    socks: Arc<Socks5Server>,
    /// Moteur BitTorrent route via le SOCKS5 (uTP only).
    pub engine: BtEngine,
    /// Transport uTP tunnelse de la lane — cote seeder d'un hidden
    /// service, les cellules `data` d'un circuit e2e `RP_SEEDER` lie
    /// y sont injectees (`inject_incoming` + `pin_circuit`).
    pub utp_transport: TunnelUdpSocket,
    /// Arret du watchdog de circuits de la lane.
    circuit_watchdog_stop: Arc<tokio::sync::watch::Sender<bool>>,
}

/// Intervalle de sondage des circuits `READY` de la lane — filet de
/// securite seulement : la reaction principale est evenementielle via
/// [`TunnelCommunity::watch_circuits`], sinon une transition
/// `READY -> detruit` plus rapide qu'un tick passerait inapercue.
const CIRCUIT_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Tick de `PingChurn.take_step` (strategies pyipv8 : toutes les
/// 0,5 s ; les cadences reelles des pings sont gatees en interne par
/// `PING_INTERVAL`).
const DHT_STEP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// `register_task("node_maintenance", interval=60)` Python.
const DHT_NODE_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
/// `register_task("value_maintenance", interval=3600)` Python.
const DHT_VALUE_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);
/// `register_task("token_maintenance", interval=300)` Python.
const DHT_TOKEN_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// `walker_interval` par defaut (`ipv8.walker_interval` Tribler : 0,5 s).
pub const DEFAULT_WALKER_INTERVAL: f64 = 0.5;
/// `TunnelSettings.min_circuits` par defaut (pyipv8/Tribler et
/// `TunnelCommunityConfig::default()`).
pub const DEFAULT_MIN_CIRCUITS: u32 = 3;

/// `TunnelSettings.max_circuits` pyipv8 (`tribler_config`
/// `tunnel_community/max_circuits`).
pub const DEFAULT_MAX_CIRCUITS: u32 = 8;

/// Taille max du cache de pairs persists (`ipv8_peers`) — extension
/// Rust : pyipv8 ne persiste pas `Network`, on borne a des centaines
/// d'entrees, les plus fraichement vues.
pub const DEFAULT_PEER_CACHE_MAX: usize = 512;
/// Retention d'un pair cache (7 jours) : au-dela, l'adresse est
/// vraisemblablement reattribute (NAT dynamique).
pub const DEFAULT_PEER_CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// Cadence du snapshot `Network` -> `ipv8_peers` (2 min) — un lot par
/// intervalle plutot qu'une ecriture par pair decouvert.
pub const DEFAULT_PEER_PERSIST_INTERVAL_SECS: u64 = 120;

/// Tache de maintenance DHT (`PingChurn.take_step` +
/// `node_maintenance`/`value_maintenance`/`token_maintenance` +
/// propagation de `my_estimated_wan` depuis la discovery, car Python
/// partage le meme `my_peer` entre communautes). Retourne l'arret.
fn spawn_dht_maintenance(
    dht: Arc<DhtCommunity>,
    discovery: Arc<DiscoveryCommunity>,
    tasks: crate::asyncio::TaskRegistry,
) -> tokio::sync::watch::Sender<bool> {
    // `register_task(...)` de `DHTCommunity.unload` Python — les
    // trois taches partagent la meme boucle ici mais restent
    // declarees separement dans le registre (`/asyncio/tasks`).
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "node_maintenance",
        Some(DHT_NODE_MAINT_INTERVAL.as_secs_f64()),
    );
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "value_maintenance",
        Some(DHT_VALUE_MAINT_INTERVAL.as_secs_f64()),
    );
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "token_maintenance",
        Some(DHT_TOKEN_MAINT_INTERVAL.as_secs_f64()),
    );
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        // Premier `token_maintenance` immediat (community.py Python :
        // appele avant le register_task, au cas ou des requetes
        // arriveraient avant le premier tick).
        dht.token_maintenance();
        let mut step = tokio::time::interval(DHT_STEP_INTERVAL);
        let mut node = tokio::time::interval(DHT_NODE_MAINT_INTERVAL);
        let mut value = tokio::time::interval(DHT_VALUE_MAINT_INTERVAL);
        let mut token = tokio::time::interval(DHT_TOKEN_MAINT_INTERVAL);
        loop {
            tokio::select! {
                _ = rx.changed() => break,
                _ = step.tick() => dht.step(),
                _ = node.tick() => dht.node_maintenance().await,
                _ = value.tick() => dht.value_maintenance(),
                _ = token.tick() => dht.token_maintenance(),
            }
            // `my_peer` Python est partage entre Discovery et DHT :
            // quand l'introduction apprend notre WAN, le DHT le voit.
            let wan = discovery.my_estimated_wan();
            if !wan.is_unspecified() {
                dht.set_my_wan(wan);
            }
            let lan = discovery.my_estimated_lan();
            if !lan.is_unspecified() {
                dht.set_my_lan(lan);
            }
        }
        tracing::debug!("maintenance DHT arretee");
    });
    tx
}

/// Watchdog de circuits d'une lane anonyme : la lane est
/// **indisponible tant qu'aucun circuit `READY` a `hops` sauts
/// n'existe** — la portee `"circuits"` est engagee des la creation
/// (fail-closed : un `add`/`resume` avant le premier circuit est
/// refuse par `guard()` au lieu d'attendre un CONNECT voue a
/// l'echec) et tant que `ready_circuits_of_hops(hops)` est vide — le
/// meme predicat que la selection de circuit donnees du serveur
/// SOCKS5, donc un circuit d'un autre nombre de sauts ne desarme pas
/// cette lane.
///
/// Distinct du watchdog proxy du moteur : le listener SOCKS5 local
/// peut rester joignable alors que tous les circuits sont morts —
/// `proxy joignable != circuit disponible`.
fn spawn_circuit_watchdog(
    tunnel: Arc<TunnelCommunity>,
    hops: usize,
    ks: Arc<tribler_network_policy::kill_switch::KillSwitch>,
    circuit_bounds: Arc<(AtomicUsize, AtomicUsize)>,
    engine: BtEngine,
) -> Arc<tokio::sync::watch::Sender<bool>> {
    // Fail-closed des la creation de la lane (avant tout circuit).
    ks.engage_scoped("circuits", format!("aucun circuit READY a {hops} sauts"));
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let mut changes = tunnel.watch_circuits();
    tokio::spawn(async move {
        // `monitor_downloads` de `TriblerTunnelCommunity` :
        // `circuits_needed[hops] = clamp(downloads actifs,
        // min_circuits, max_circuits)`. Ecart assumé : la lane garde
        // `min_circuits` en veille meme sans telechargement actif
        // (Python ne demande des circuits qu'aux hops utilises) — un
        // premier `add` n'attend pas la construction a froid.
        let circuits_needed = |engine: &BtEngine| {
            let actifs = engine
                .list()
                .iter()
                .filter(|d| {
                    matches!(
                        d.state,
                        tribler_bittorrent::DownloadState::Downloading
                            | tribler_bittorrent::DownloadState::Seeding
                            | tribler_bittorrent::DownloadState::Initializing
                    )
                })
                .count();
            // Bornes relues a chaud (`POST /api/settings` ->
            // `set_circuit_bounds`), comme `self.settings` Python.
            actifs.clamp(
                circuit_bounds.0.load(Ordering::Relaxed),
                circuit_bounds.1.load(Ordering::Relaxed).max(1),
            )
        };
        let mut tick = tokio::time::interval(CIRCUIT_PROBE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Tentative immediate de construction de circuit(s). Plusieurs
        // circuits en parallele (`min_circuits`, `TunnelSettings`
        // pyipv8) sont necessaires : un unique circuit dont le
        // dernier saut annonce `EXIT_HTTP` sans le servir reellement
        // (pair instable/mensonger du reseau public) bloque
        // definitivement la lane sinon, faute d'alternative pour le
        // selecteur SOCKS5 (`select_http_circuit`).
        let _ = tunnel
            .build_circuits_if_needed(hops, circuits_needed(&engine))
            .await;
        // Premier tick immediat : absorbe (l'evaluation initiale est
        // deja faite ci-dessus).
        tick.tick().await;
        loop {
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {
                    let _ = tunnel
                        .build_circuits_if_needed(hops, circuits_needed(&engine))
                        .await;
                }
            }
            if tunnel.ready_circuits_of_hops(hops).is_empty() {
                ks.engage_scoped("circuits", format!("aucun circuit READY a {hops} sauts"));
            } else {
                ks.release_scoped("circuits");
            }
        }
    });
    Arc::new(stop_tx)
}

/// `ContentProvider` adosse a la base : santes depuis `torrent_state`,
/// metadonnees depuis `channel_node`, serialisation `.mdblob` + LZ4.
struct SessionContentProvider {
    db: Arc<Database>,
    /// `Notification.torrent_metadata_added` Python : chaque entree
    /// inseree est notifiee (nourrit l'augmenteur de recherche).
    notifier: crate::notifier::Notifier,
    /// `remote_queries_in_progress` Python : une seule requete
    /// `txt_filter` distante a la fois (`process_rpc_query_rate_limited`).
    remote_queries_in_progress: std::sync::atomic::AtomicUsize,
    /// `max_response_size` Python (100) : entrees max par select.
    max_response_size: usize,
    /// `maximum_payload_size` Python (1300) : octets d'entrees max
    /// par chunk de reponse.
    max_payload_size: usize,
}

/// `deprecated_parameters` Python : ces parametres de select sont
/// rejetes (reponse archive vide).
const DEPRECATED_SELECT_PARAMS: [&str; 3] = ["subscribed", "attribute_ranges", "complete_channel"];

impl SessionContentProvider {
    /// Convertit une ligne `channel_node` en entree `.mdblob`
    /// pre-signee (signature conservee telle quelle).
    fn row_to_entry(
        row: &tribler_db::models::ChannelNodeRow,
    ) -> Option<tribler_format::mdblob::MetadataEntry> {
        use tribler_format::mdblob::*;
        let mut public_key = [0u8; 64];
        if row.public_key.len() != 64 {
            return None;
        }
        public_key.copy_from_slice(&row.public_key);
        let mut infohash = [0u8; 20];
        if row.infohash.len() != 20 {
            return None;
        }
        infohash.copy_from_slice(&row.infohash);
        let signature = row
            .signature
            .as_deref()
            .and_then(|s| <[u8; 64]>::try_from(s).ok())
            .unwrap_or([0u8; 64]);
        let node = ChannelNodePayload {
            header: SignedPayloadHeader::new(
                row.metadata_type as u16,
                row.reserved_flags as u16,
                public_key,
            )
            .with_signature(signature),
            id: row.id_ as u64,
            origin_id: row.origin_id as u64,
            timestamp: row.timestamp as u64,
        };
        let torrent = TorrentMetadataPayload {
            node,
            infohash,
            size: row.size.max(0) as u64,
            torrent_date: row.torrent_date.max(0) as u32,
            title: row.title.clone(),
            tags: row.tags.clone(),
            tracker_info: row.tracker_info.clone(),
        };
        Some(match row.metadata_type as u16 {
            types::CHANNEL_TORRENT => MetadataEntry::ChannelTorrent(ChannelMetadataPayload {
                torrent,
                num_entries: 0,
                start_timestamp: row.timestamp.max(0) as u64,
            }),
            _ => MetadataEntry::RegularTorrent(torrent),
        })
    }

    /// Corps du select distant (cf. `ContentProvider::remote_select`).
    fn remote_select_inner(&self, query: &serde_json::Value) -> Vec<Vec<u8>> {
        // `sanitize_query` : `last` borne a `first + max_response_size`.
        let first = query.get("first").and_then(|v| v.as_u64()).unwrap_or(0);
        let last = query
            .get("last")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(first + self.max_response_size as u64))
            .unwrap_or(first + self.max_response_size as u64);

        let rows = self
            .db
            .with(|conn| select_rows(conn, query, first, last))
            .unwrap_or_default();

        // `entries_to_chunk` Python : entrees serialisees groupees
        // par budget de `maximum_payload_size`, compressees LZ4 par
        // chunk — chaque chunk part dans un `SelectResponse` separe.
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let mut cur = Vec::new();
        for row in rows {
            let Some(entry) = Self::row_to_entry(&row) else {
                continue;
            };
            let Ok(bytes) = tribler_format::mdblob::encode_entry_presigned(&entry) else {
                continue;
            };
            if cur.len() + bytes.len() > self.max_payload_size && !cur.is_empty() {
                chunks.push(lz4_frame(&cur));
                cur.clear();
            }
            cur.extend_from_slice(&bytes);
        }
        if !cur.is_empty() || chunks.is_empty() {
            chunks.push(lz4_frame(&cur));
        }
        chunks
    }
}

/// `lz4.frame.compress(b"")` Python (`LZ4_EMPTY_ARCHIVE`).
fn lz4_frame(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    if enc.write_all(data).is_err() {
        return Vec::new();
    }
    enc.finish().unwrap_or_default()
}

impl ContentProvider for SessionContentProvider {
    /// `get_random_torrents`/`get_popular_torrents` : santes jointes
    /// `channel_node`/`torrent_state`.
    fn healths_for(&self, request_type: u8) -> Vec<HealthInfo> {
        let popular = request_type == HEALTH_REQUEST_POPULAR;
        self.db
            .with(|conn| {
                let sql = if popular {
                    "SELECT n.infohash, t.seeders, t.leechers, t.last_check, n.tracker_info
                     FROM channel_node n JOIN torrent_state t ON t.rowid = n.health_rowid
                     WHERE n.infohash != '' ORDER BY t.seeders DESC LIMIT 20"
                } else {
                    "SELECT n.infohash, t.seeders, t.leechers, t.last_check, n.tracker_info
                     FROM channel_node n JOIN torrent_state t ON t.rowid = n.health_rowid
                     WHERE n.infohash != '' ORDER BY RANDOM() LIMIT 20"
                };
                let mut stmt = conn.prepare(sql)?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?;
                Ok(rows
                    .filter_map(|r| r.ok())
                    .filter_map(|(ih, s, l, lc, tr)| {
                        let mut infohash = [0u8; 20];
                        if ih.len() != 20 {
                            return None;
                        }
                        infohash.copy_from_slice(&ih);
                        Some(HealthInfo {
                            infohash,
                            seeders: s.max(0) as u32,
                            leechers: l.max(0) as u32,
                            last_check: lc.max(0) as u64,
                            tracker: tr,
                        })
                    })
                    .collect())
            })
            .unwrap_or_default()
    }

    /// `process_torrents_health` : met a jour `torrent_state` ;
    /// retourne les infohashes inconnus (a resoudre par select).
    fn process_health(&self, healths: &[HealthInfo]) -> Vec<[u8; 20]> {
        self.db
            .with(|conn| {
                let mut unknown = Vec::new();
                for h in healths {
                    tribler_db::health::upsert_torrent_state(conn, &h.infohash)?;
                    tribler_db::health::update_torrent_health(
                        conn,
                        &h.infohash,
                        h.seeders as i64,
                        h.leechers as i64,
                        h.last_check as i64,
                        false,
                    )?;
                    if tribler_db::channel::get_by_infohash(conn, &h.infohash)?.is_none() {
                        unknown.push(h.infohash);
                    }
                }
                Ok(unknown)
            })
            .unwrap_or_default()
    }

    /// Select distant : parametres JSON (`txt_filter`, `infohash`,
    /// `infohash_set`, `first`/`last`, `metadata_type`, `channel_pk`,
    /// `origin_id`, `max_rowid`, `hide_xxx`) → chunks `.mdblob`
    /// compresses LZ4 (`send_db_results` Python : un `SelectResponse`
    /// par chunk, archive vide si rien).
    fn remote_select(&self, json: &[u8]) -> Vec<Vec<u8>> {
        // `parse_parameters` Python : JSON invalide -> pas de
        // reponse exploitable (archive vide = `LZ4_EMPTY_ARCHIVE`).
        let Ok(query) = serde_json::from_slice::<serde_json::Value>(json) else {
            return vec![lz4_frame(&[])];
        };
        // `deprecated_parameters` : rejet (Python renvoie l'archive vide).
        if DEPRECATED_SELECT_PARAMS
            .iter()
            .any(|k| query.get(k).is_some())
        {
            tracing::warn!(%query, "remote select avec parametres deprecies");
            return vec![lz4_frame(&[])];
        }
        // `process_rpc_query_rate_limited` : une seule requete
        // `txt_filter` a la fois, les autres sont ignorees.
        let rate_limited = query.get("txt_filter").is_some();
        if rate_limited
            && self
                .remote_queries_in_progress
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                > 0
        {
            self.remote_queries_in_progress
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return vec![lz4_frame(&[])];
        }
        let result = self.remote_select_inner(&query);
        if rate_limited {
            self.remote_queries_in_progress
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        result
    }

    /// `process_compressed_mdblob` : decompresse LZ4, parse les
    /// entrees et les insere dans `channel_node`. Retourne les
    /// `to_simple_dict()` des objets NOUVEAUX (`ObjState.NEW_OBJECT` —
    /// la dedup `(public_key, id_)` de `channel::insert` fait foi).
    fn process_select_response(&self, blob: &[u8]) -> Vec<serde_json::Value> {
        use std::io::Read;
        let mut data = Vec::new();
        if lz4_flex::frame::FrameDecoder::new(blob)
            .read_to_end(&mut data)
            .is_err()
        {
            return Vec::new();
        }
        let Ok(entries) = tribler_format::mdblob::parse_blob(&data) else {
            return Vec::new();
        };
        let (results, new_titles) = self
            .db
            .with(|conn| {
                let mut results = Vec::new();
                let mut new_titles = Vec::new();
                for e in &entries {
                    let Some(row) = entry_to_row(e) else {
                        continue;
                    };
                    if tribler_db::channel::insert(conn, &row)?.is_none() {
                        // `DUPLICATE_OBJECT` : exclu des `results`
                        // (comme `notify_gui` Python).
                        continue;
                    }
                    results.push(simple_dict(conn, &row)?);
                    if !row.title.is_empty() {
                        new_titles.push((hex::encode(&row.infohash), row.title.clone()));
                    }
                }
                Ok((results, new_titles))
            })
            .unwrap_or_default();
        // `torrent_metadata_added` : le notifier consomme aussi les
        // titres pour l'apprentissage de l'augmenteur.
        for (infohash, title) in new_titles {
            self.notifier
                .notify(crate::notifier::Notification::TorrentMetadataCreated { infohash, title });
        }
        results
    }

    /// `(version, plateforme)` pour `VersionResponse` —
    /// `f"Tribler {version}"` + `sys.platform` Python.
    fn version_info(&self) -> (String, String) {
        let platform = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        (
            format!("Tribler {}", env!("CARGO_PKG_VERSION")),
            platform.into(),
        )
    }
}

/// Convertit une entree `.mdblob` en ligne `channel_node` (insertion).
fn entry_to_row(
    entry: &tribler_format::mdblob::MetadataEntry,
) -> Option<tribler_db::models::ChannelNodeRow> {
    use tribler_format::mdblob::*;
    let (node, torrent) = match entry {
        MetadataEntry::RegularTorrent(t) => (&t.node, Some(t)),
        MetadataEntry::ChannelTorrent(c) => (&c.torrent.node, Some(&c.torrent)),
        _ => return None,
    };
    let t = torrent?;
    Some(tribler_db::models::ChannelNodeRow {
        rowid: 0,
        infohash: t.infohash.to_vec(),
        size: t.size as i64,
        torrent_date: t.torrent_date as i64,
        tracker_info: t.tracker_info.clone(),
        title: t.title.clone(),
        tags: t.tags.clone(),
        metadata_type: node.header.metadata_type as i64,
        reserved_flags: node.header.reserved_flags as i64,
        origin_id: node.origin_id as i64,
        public_key: node.header.public_key.to_vec(),
        id_: node.id as i64,
        timestamp: node.timestamp as i64,
        signature: Some(node.header.signature.to_vec()),
        added_on: now_unix() as i64,
        status: 1,
        xxx: 0.0,
        health_rowid: None,
        tag_processor_version: 0,
        health_seeders: None,
        health_leechers: None,
        health_last_check: None,
    })
}

/// `TorrentMetadata.to_simple_dict()` Python : la forme JSON envoyee
/// dans `remote_query_results.results`/`local_query_results.results`.
fn simple_dict(
    conn: &rusqlite::Connection,
    row: &tribler_db::models::ChannelNodeRow,
) -> rusqlite::Result<serde_json::Value> {
    let (seeders, leechers, last_check) = conn
        .query_row(
            "SELECT seeders, leechers, last_check FROM torrent_state
             WHERE infohash = ?1",
            rusqlite::params![&row.infohash],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap_or((0, 0, 0));
    Ok(serde_json::json!({
        "name": row.title,
        "category": row.tags,
        "infohash": hex::encode(&row.infohash),
        "size": row.size,
        "num_seeders": seeders,
        "num_leechers": leechers,
        "last_tracker_check": last_check,
        "created": row.torrent_date,
        "tag_processor_version": row.tag_processor_version,
        "type": row.metadata_type,
        "id": row.id_,
        "origin_id": row.origin_id,
        "public_key": hex::encode(&row.public_key),
        "status": row.status,
        // `tracker_info_list` Python : vide cote metadonnees distantes
        // (`tracker_info` est deprecie en 8.x).
        "trackers": [],
    }))
}

/// Secondes Unix courantes.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Champ binaire hex d'une requete select (`binary_fields` Python :
/// `infohash`, `channel_pk` arrives en hex).
fn json_hex_bytes(query: &serde_json::Value, key: &str) -> Option<Vec<u8>> {
    query
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| hex::decode(s.trim_matches('"')).ok())
}

/// `metadata_type` Python : accepte entier, chaine ou liste
/// (`convert_to_json` convertit la chaine en `[int]`).
fn json_metadata_types(query: &serde_json::Value) -> Option<Vec<i64>> {
    let v = query.get("metadata_type")?;
    let one = |v: &serde_json::Value| -> Option<i64> {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    match v {
        serde_json::Value::Array(arr) => Some(arr.iter().filter_map(one).collect()),
        other => one(other).map(|m| vec![m]),
    }
}

/// Filtre les lignes `channel_node` selon la requete JSON distante
/// (`get_entries` Python : `txt_filter` FTS -> AND de LIKE,
/// `infohash`/`infohash_set`, `channel_pk`, `origin_id`,
/// `metadata_type`, `max_rowid`, `hide_xxx`), puis tranche
/// `first`..`last` de `sanitize_query`.
fn select_rows(
    conn: &rusqlite::Connection,
    query: &serde_json::Value,
    first: u64,
    last: u64,
) -> tribler_db::Result<Vec<tribler_db::models::ChannelNodeRow>> {
    // `txt_filter` arrive deja formate FTS (`"a" "b"`) : il part en
    // `FtsIndex MATCH` tel quel ; `terms` sert de repli `LIKE`.
    let txt = query
        .get("txt_filter")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let params = tribler_db::channel::SelectParams {
        txt_filter: txt.clone(),
        terms: txt
            .as_deref()
            .map(crate::queries::fts_terms)
            .unwrap_or_default(),
        metadata_types: json_metadata_types(query),
        infohash: json_hex_bytes(query, "infohash"),
        infohash_set: query
            .get("infohash_set")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| hex::decode(s.trim_matches('"')).ok())
                    .collect()
            })
            .unwrap_or_default(),
        channel_pk: json_hex_bytes(query, "channel_pk"),
        origin_id: query.get("origin_id").and_then(|v| v.as_i64()),
        id_: query.get("id").and_then(|v| v.as_i64()),
        max_rowid: query.get("max_rowid").and_then(|v| v.as_i64()),
        hide_xxx: query
            .get("hide_xxx")
            .and_then(|v| {
                v.as_bool()
                    .or_else(|| v.as_str().map(|s| s == "1" || s == "true"))
            })
            .unwrap_or(false),
        sort_by: query
            .get("sort_by")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        sort_desc: query
            .get("sort_desc")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        category: query
            .get("category")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tags: query
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        first,
        last: Some(last),
        ..Default::default()
    };
    tribler_db::channel::select_entries(conn, &params)
}

/// Stack IPv8 de session (endpoint + communities + lanes anonymes).
pub struct Ipv8Stack {
    /// Endpoint UDP IPv8 partage.
    pub endpoint: Arc<UdpEndpoint>,
    /// Registre de pairs IPv8.
    pub network: Arc<Network>,
    /// Community de decouverte de pairs.
    pub discovery: Arc<DiscoveryCommunity>,
    /// Community de decouverte de contenu (`None` si
    /// `content_discovery_community/enabled = false`).
    pub content_discovery: Option<Arc<ContentDiscoveryCommunity>>,
    /// `TunnelCommunity` (presente si `enable_anonymity`).
    pub tunnel: Option<Arc<TunnelCommunity>>,
    /// `DHTDiscoveryCommunity` (presente si `enable_dht` —
    /// `dht_discovery/enabled` Python). Sert aussi de `DHTCommunity`
    /// pour les routes `/api/ipv8/dht/*`.
    pub dht: Option<Arc<DhtCommunity>>,
    /// Cle IPv8 de la session (persistee dans `state_dir`).
    key: LibNaClSecretKey,
    /// Arret de la tache de maintenance DHT.
    dht_maintenance_stop: Option<tokio::sync::watch::Sender<bool>>,
    /// Moteurs anonymes par nombre de sauts (1..=3).
    anon_lanes: Mutex<HashMap<usize, AnonLane>>,
    /// Config moteur de base (pour creer les lanes anonymes).
    engine_config: EngineConfig,
    /// Repertoire de telechargement par defaut.
    downloads_dir: PathBuf,
    /// `state_dir` (dossiers de persistance fastresume des lanes).
    state_dir: PathBuf,
    /// Registre partage des taches nommees (`/api/ipv8/asyncio/tasks`).
    tasks: crate::asyncio::TaskRegistry,
    /// Bornes `min_circuits`/`max_circuits` partagees avec les
    /// watchdogs de lanes : `POST /api/settings` les met a jour a
    /// chaud (`set_circuit_bounds`) — Python relit `self.settings`
    /// a chaque tick de `monitor_downloads`.
    circuit_bounds: Arc<(AtomicUsize, AtomicUsize)>,
    /// `libtorrent/socks_listen_ports` (ports SOCKS5 par lane).
    socks_listen_ports: Vec<u16>,
    /// `monitor_hidden_swarms` Python : dernier etat connu par
    /// `(hops, lookup_info_hash)` pour detecter les transitions
    /// join/leave des swarms caches.
    swarm_states: Mutex<HashMap<(usize, [u8; 20]), tribler_bittorrent::DownloadState>>,
    /// `lookup_info_hash -> (hops, info_hash reel)` : les circuits
    /// e2e notifies par `e2e_ready` portent l'info-hash de LOOKUP du
    /// swarm, ce mapping retrouve le download correspondant.
    swarm_lookup: Mutex<SwarmLookupMap>,
    /// Arrets des taches hidden-services (monitor des swarms +
    /// relais `e2e_ready`), creees a la premiere lane anonyme.
    hidden_tasks: Mutex<Vec<tokio::sync::watch::Sender<bool>>>,
    /// `Weak` auto-reference (les taches hidden-services demarrees
    /// depuis `anon_engine(&self)` n'ont pas acces a `Arc<Self>`).
    self_weak: Mutex<std::sync::Weak<Ipv8Stack>>,
}

/// Fichier de persistance des noeuds de sortie (`exitnode_cache`
/// Python) dans `state_dir` : lignes `<pubkey_hex> <addr> <flags>`.
const EXITNODE_CACHE_FILE: &str = "exitnodes.txt";

/// Cadence de `monitor_hidden_swarms` Python (poll des etats de
/// telechargement via `DownloadManager` / `get_last_download_states`).
const SWARM_MONITOR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Mapping `lookup_info_hash -> (hops, info_hash reel)` des swarms
/// caches (champ `swarm_lookup` de `Ipv8Stack`).
type SwarmLookupMap = HashMap<[u8; 20], (usize, [u8; 20])>;

impl Ipv8Stack {
    /// Cree et demarre la stack : endpoint, communities, discovery
    /// bootstrap (tache de fond). `notifier` recoit le relais
    /// `circuit_removed` -> `tunnel_removed`.
    pub async fn start(
        config: &Ipv8Config,
        state_dir: &Path,
        downloads_dir: &Path,
        engine_config: &EngineConfig,
        db: Arc<Database>,
        notifier: crate::notifier::Notifier,
        tasks: crate::asyncio::TaskRegistry,
    ) -> Result<Arc<Self>> {
        let key = load_or_create_key(&state_dir.join(IPV8_KEY_FILE))?;
        // `ipv8/interfaces` pyipv8 : `UDPIPv4` obligatoire, `UDPIPv6`
        // optionnel (socket secondaire du meme endpoint, partage des
        // listeners — `DispatcherEndpoint`).
        let bind_v6 = config.listen_addr_v6.as_deref();
        let endpoint = match UdpEndpoint::bind_dual(&config.listen_addr, bind_v6).await {
            Ok(ep) => ep,
            Err(e) if bind_v6.is_some() => {
                tracing::warn!(
                    error = %e,
                    listen_v6 = %bind_v6.unwrap_or_default(),
                    "bind UDP IPv8 v6 echoue, repli IPv4 seul"
                );
                match UdpEndpoint::bind(&config.listen_addr).await {
                    Ok(ep) => ep,
                    Err(e) => {
                        if config.listen_addr != "0.0.0.0:0" {
                            tracing::warn!(
                                error = %e,
                                listen = %config.listen_addr,
                                "bind UDP IPv8 echoue sur l'adresse configuree, repli sur 0.0.0.0:0 (port ephemere)"
                            );
                            UdpEndpoint::bind("0.0.0.0:0").await.map_err(|e2| {
                                CoreError::State(format!("bind ipv8 port ephemere: {e2}"))
                            })?
                        } else {
                            return Err(CoreError::State(format!("bind ipv8: {e}")));
                        }
                    }
                }
            }
            Err(e) => {
                if config.listen_addr != "0.0.0.0:0" {
                    tracing::warn!(
                        error = %e,
                        listen = %config.listen_addr,
                        "bind UDP IPv8 echoue sur l'adresse configuree, repli sur 0.0.0.0:0 (port ephemere)"
                    );
                    UdpEndpoint::bind("0.0.0.0:0")
                        .await
                        .map_err(|e2| CoreError::State(format!("bind ipv8 port ephemere: {e2}")))?
                } else {
                    return Err(CoreError::State(format!("bind ipv8: {e}")));
                }
            }
        };
        let endpoint: Arc<UdpEndpoint> = endpoint;
        let network = Arc::new(Network::default());
        let discovery = DiscoveryCommunity::new(
            key.clone(),
            network.clone(),
            endpoint.clone(),
            UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                endpoint
                    .local_addr()
                    .map_err(|e| CoreError::State(format!("addr ipv8: {e}")))?
                    .port(),
            )),
        )
        .await;
        // Extension Rust (bancs loopback) : `ipv8.estimated_wan` —
        // sur un mesh 100 % loopback `my_estimated_wan` ne s'apprend
        // jamais (`address_in_lan_subnets`), ce qui figerait le DHT
        // (`on_node_discovered` refuse tout noeud — comme pyipv8).
        if let Some(wan) = &config.estimated_wan {
            discovery.set_estimated_wan(wan.clone());
        }
        // `ContentDiscoveryComponent` Python : cree seulement si
        // `content_discovery_community/enabled` (la recherche distante
        // et `/api/search` retournent alors 503-vide cote REST).
        let content_discovery = if config.enable_content_discovery {
            // `ContentDiscoverySettings` Python : defauts filaires
            // (gossip 5 s, max 20 pairs, TTL 10 s, 10 paquets max).
            let cd_settings = tribler_ipv8::content_discovery::ContentDiscoverySettings::default();
            let provider = Arc::new(SessionContentProvider {
                db: db.clone(),
                notifier: notifier.clone(),
                remote_queries_in_progress: std::sync::atomic::AtomicUsize::new(0),
                max_response_size: 100,
                max_payload_size: 1300,
            });
            Some(
                ContentDiscoveryCommunity::new(
                    key.clone(),
                    network.clone(),
                    endpoint.clone(),
                    provider,
                    cd_settings,
                    discovery.clone(),
                )
                .await,
            )
        } else {
            None
        };
        let dht = if config.enable_dht {
            // `DHTDiscoveryCommunity` Python : `my_estimated_wan`
            // commence non-specifie et est appris par introduction ;
            // `my_estimated_lan` = notre adresse d'ecoute.
            let lan = UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                endpoint
                    .local_addr()
                    .map_err(|e| CoreError::State(format!("addr ipv8: {e}")))?
                    .port(),
            ));
            Some(
                DhtCommunity::new(
                    key.clone(),
                    UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::UNSPECIFIED,
                        0,
                    )),
                    lan,
                    endpoint.clone(),
                )
                .await,
            )
        } else {
            None
        };
        let tunnel = if config.enable_anonymity {
            let community_id = if config.tribler_tunnel_community {
                TRIBLER_TUNNEL_COMMUNITY_ID
            } else {
                tribler_tunnel::TUNNEL_COMMUNITY_ID
            };
            let t = TunnelCommunity::new_with_id(
                key.clone(),
                network.clone(),
                endpoint.clone(),
                tribler_tunnel::settings::TunnelSettings {
                    peer_flags: config.peer_flags,
                    min_circuits: config.min_circuits.max(1) as usize,
                    max_circuits: config.max_circuits.max(1) as usize,
                    intro_point_peer: config.intro_point_peer.clone(),
                    data_exit_peer: config.data_exit_peer.clone(),
                    ..tribler_tunnel::settings::TunnelSettings::default()
                },
                community_id,
            )
            .await;
            // `my_peer` Python est partage entre overlays : la
            // tunnel-community emprunte les estimations WAN/LAN de la
            // discovery pour ses introductions et punctures.
            t.set_discovery(discovery.clone());
            // `dht_provider` (`out["dht_provider"] =
            // DHTCommunityProvider(...)` du composant Tribler) :
            // annonces/lookups des points d'introduction des swarms
            // caches dans la `DHTDiscoveryCommunity`.
            if let Some(d) = &dht {
                t.set_dht_provider(d.clone());
            }
            // `register_task("do_circuits")`/`do_ping`/
            // `do_peer_discovery` Python : maintenance des circuits
            // (nettoyage, keepalive, lookups de swarms caches).
            tasks.register(Some("TriblerTunnelCommunity"), "maintenance", None);
            let t_maint = t.clone();
            tokio::spawn(async move {
                t_maint.run_maintenance().await;
            });
            // `load_exit_nodes` Python : reintroduit les noeuds de
            // sortie connus de la session precedente dans `Network`
            // puis les re-sollicite (`send_introduction_request`).
            for (pk, addr, flags) in load_exitnode_cache(&state_dir.join(EXITNODE_CACHE_FILE)) {
                t.register_exit_peer(&pk, addr, flags);
                let t = t.clone();
                tokio::spawn(async move {
                    let _ = t.send_introduction_request(&UdpAddress::from(addr)).await;
                });
            }
            Some(t)
        } else {
            None
        };

        // Stores PEX persistes (`tunnel_pex`) : on redevient point
        // d'introduction des swarms connus des le demarrage — le
        // hidden seeding reste joignable sans attendre un nouveau
        // `establish-intro` du seeder.
        if let Some(t) = &tunnel {
            let pex_rows = db
                .call("ipv8.pex_list", tribler_db::pex::list)
                .await
                .unwrap_or_default();
            if !pex_rows.is_empty() {
                type Grouped = HashMap<
                    [u8; 20],
                    (
                        Vec<tribler_tunnel::routing::IntroductionPoint>,
                        Vec<Vec<u8>>,
                    ),
                >;
                let mut grouped = Grouped::new();
                for r in pex_rows {
                    let Ok(ih) = <[u8; 20]>::try_from(r.info_hash.as_slice()) else {
                        continue;
                    };
                    let entry = grouped.entry(ih).or_default();
                    if r.own {
                        entry.1.push(r.seeder_pk);
                    } else if let Ok(sa) = r.address.parse::<SocketAddr>() {
                        entry.0.push(tribler_tunnel::routing::IntroductionPoint {
                            address: UdpAddress::from(sa),
                            peer_key: r.peer_key,
                            seeder_pk: r.seeder_pk,
                            source: r.source as u8,
                            last_seen_secs: r.last_seen as u64,
                        });
                    }
                }
                let n = grouped.len();
                t.pex_restore(
                    grouped
                        .into_iter()
                        .map(|(ih, (learned, announces))| (ih, learned, announces))
                        .collect(),
                );
                tracing::info!(swarms = n, "stores PEX restaures depuis la base");
            }
        }

        // Relais `circuit_removed` -> `Notification::TunnelRemoved`
        // (tribler-tunnel ne depend pas de tribler-core : pont par
        // canal broadcast, equivalent du Notifier partage Python).
        if let Some(t) = &tunnel {
            let mut rx = t.watch_circuit_removed();
            let notifier = notifier.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => notifier.notify(crate::notifier::Notification::TunnelRemoved {
                            circuit_id: ev.circuit_id,
                            circuit_class: ev.circuit_class.to_string(),
                            bytes_up: ev.bytes_up,
                            bytes_down: ev.bytes_down,
                            uptime_secs: ev.uptime_secs,
                            additional_info: ev.additional_info,
                        }),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
            });
        }

        // `enable_overlay_statistics(enable=True, all_overlays=True)`
        // de `session.py` : Tribler active le suivi des stats pour
        // toutes les communities des le demarrage.
        let stats_prefixes = {
            let mut v = vec![tribler_ipv8::prefix_of(
                &tribler_ipv8::discovery::DISCOVERY_COMMUNITY_ID,
            )];
            if content_discovery.is_some() {
                v.push(tribler_ipv8::prefix_of(
                    &tribler_ipv8::CONTENT_DISCOVERY_COMMUNITY_ID,
                ));
            }
            if let Some(t) = &tunnel {
                v.push(tribler_ipv8::prefix_of(&t.community_id()));
            }
            if dht.is_some() {
                v.push(tribler_ipv8::prefix_of(
                    &tribler_ipv8::dht::DHT_COMMUNITY_ID,
                ));
            }
            v
        };
        for prefix in stats_prefixes {
            endpoint.enable_community_statistics(prefix, true).await;
        }

        // Tache de reception de l'endpoint (dispatch prefixe) —
        // `ensure_future(endpoint.run())` Python (tache anonyme).
        tasks.register(None, "endpoint", None);
        let ep = endpoint.clone();
        tokio::spawn(async move {
            if let Err(e) = ep.run().await {
                tracing::error!(error = %e, "endpoint ipv8 termine");
            }
        });

        // Maintenance DHT en tache de fond (arret via `stop`).
        let dht_maintenance_stop = dht
            .clone()
            .map(|d| spawn_dht_maintenance(d, discovery.clone(), tasks.clone()));

        // Cache de pairs verifies (`ipv8_peers`) : recharge les plus
        // frais dans `Network` AVANT le bootstrap — les walks peuvent
        // viser des pairs connus immediatement, sans attendre la
        // resolution DNS ni un premier cycle d'introduction.
        let peer_cutoff = now_unix().saturating_sub(config.peer_cache_max_age_secs) as i64;
        let peer_cache_max = config.peer_cache_max;
        let restored = db
            .call("ipv8.peers_restore", move |c| {
                tribler_db::peers::list_peers(c, peer_cutoff, peer_cache_max)
            })
            .await
            .unwrap_or_default();
        let mut warm_addrs: Vec<UdpAddress> = Vec::new();
        for row in &restored {
            if let Ok(sa) = row.address.parse::<SocketAddr>() {
                let addr = UdpAddress::from(sa);
                let mut peer = match Peer::new(row.public_key.clone(), Some(addr.clone())) {
                    Some(p) => p,
                    None => continue,
                };
                peer.new_style_intro = row.new_style;
                network.add_verified(peer);
                warm_addrs.push(addr);
            }
        }
        if !warm_addrs.is_empty() {
            tracing::info!(
                restored = warm_addrs.len(),
                "pairs IPv8 restaures depuis le cache persistant"
            );
        }

        // Snapshot periodique `Network` -> `ipv8_peers` : une ecriture
        // en lot par intervalle plutot qu'un upsert par pair decouvert
        // (le WAL + `call` gardent la boucle d'evenements fluide).
        tasks.register(
            None,
            "ipv8_peer_cache",
            Some(config.peer_persist_interval_secs as f64),
        );
        {
            let network = network.clone();
            let db = db.clone();
            let tunnel_cache = tunnel.clone();
            let interval = std::time::Duration::from_secs(config.peer_persist_interval_secs.max(5));
            let max = config.peer_cache_max;
            let max_age = config.peer_cache_max_age_secs;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let now = now_unix() as i64;
                    let cutoff = now.saturating_sub(max_age as i64);
                    // Stores PEX : annonces propres + points appris
                    // (meme snapshot que `ipv8_peers`, meme ecriture).
                    let pex_rows: Vec<tribler_db::PexRow> = tunnel_cache
                        .as_ref()
                        .map(|t| {
                            t.pex_dump()
                                .into_iter()
                                .flat_map(|(ih, learned, announces)| {
                                    let mut v = Vec::with_capacity(learned.len() + announces.len());
                                    for ip in learned {
                                        v.push(tribler_db::PexRow {
                                            info_hash: ih.to_vec(),
                                            own: false,
                                            peer_key: ip.peer_key,
                                            seeder_pk: ip.seeder_pk,
                                            address: ip
                                                .address
                                                .to_socket_addr()
                                                .map(|s| s.to_string())
                                                .unwrap_or_default(),
                                            source: ip.source as i64,
                                            last_seen: ip.last_seen_secs as i64,
                                        });
                                    }
                                    for pk in announces {
                                        v.push(tribler_db::PexRow {
                                            info_hash: ih.to_vec(),
                                            own: true,
                                            peer_key: Vec::new(),
                                            seeder_pk: pk,
                                            address: String::new(),
                                            source: 0,
                                            last_seen: 0,
                                        });
                                    }
                                    v
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let write_pex = tunnel_cache.is_some();
                    let rows: Vec<tribler_db::Ipv8PeerRow> = network
                        .verified_peers()
                        .iter()
                        .filter_map(|p| {
                            let sa = p.address.as_ref()?.to_socket_addr()?;
                            if sa.ip().is_unspecified() {
                                return None;
                            }
                            Some(tribler_db::Ipv8PeerRow {
                                public_key: p.public_key_bin.clone(),
                                address: sa.to_string(),
                                last_seen: now,
                                new_style: p.new_style_intro,
                            })
                        })
                        .collect();
                    let _ = db
                        .call("ipv8.peers_pex_persist", move |c| {
                            if !rows.is_empty() {
                                tribler_db::peers::upsert_batch(c, &rows, now)?;
                                tribler_db::peers::prune(c, cutoff, max)?;
                            }
                            if write_pex {
                                tribler_db::pex::replace_all(c, &pex_rows)?;
                            }
                            Ok(())
                        })
                        .await;
                }
            });
        }

        // Bootstrap discovery en tache de fond (resolution DNS + IP).
        // `register_anonymous_task("bootstrap", ...)` Python. Les
        // adresses du cache sont marchees en plus des noeuds
        // d'amorcage (le cache seul suffit a amorcer).
        let bootstrap_peers_config = config.bootstrap_peers.clone();
        if !bootstrap_peers_config.is_empty() || !warm_addrs.is_empty() {
            tasks.register(None, "bootstrap", None);
            let d = discovery.clone();
            let dht = dht.clone();
            let cd = content_discovery.clone();
            let tunnel_for_walk = tunnel.clone();
            let walker_interval = std::time::Duration::from_secs_f64(config.walker_interval);
            tokio::spawn(async move {
                let mut peers = Vec::new();
                for peer_str in &bootstrap_peers_config {
                    if let Ok(sa) = peer_str.parse::<SocketAddr>() {
                        peers.push(UdpAddress::from(sa));
                    } else if let Ok(mut resolved) =
                        tokio::net::lookup_host(peer_str.as_str()).await
                    {
                        if let Some(sa) = resolved.next() {
                            peers.push(UdpAddress::from(sa));
                        }
                    }
                }
                // Pairs caches : ajoutes aux cibles de marche (dedup
                // — un noeud d'amorcage peut aussi etre un pair connu).
                for a in warm_addrs {
                    if !peers.contains(&a) {
                        peers.push(a);
                    }
                }
                // Intro-requests DHT vers les noeuds d'amorcage : la
                // marche discovery peuple l'annuaire partage, le DHT
                // envoie ses propres pings pour remplir ses tables.
                if let Some(dht) = &dht {
                    for addr in &peers {
                        let _ = dht.walk_to(addr).await;
                    }
                }
                if !peers.is_empty() {
                    tracing::info!(
                        count = peers.len(),
                        "bootstrap IPv8 demarre vers les noeuds d'amorcage"
                    );
                    // `RandomWalk` par overlay (BaseLauncher Python
                    // donne `RandomWalk(20)` a toutes les communities
                    // Tribler) : chaque overlay marche sous son propre
                    // prefixe, sinon l'overlay reste vide — le gossip
                    // content-discovery n'a personne a qui parler et
                    // la recherche distante est muette.
                    if let Some(cd) = &cd {
                        let cd = cd.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            cd.run(peers, walker_interval).await;
                        });
                    }
                    if let Some(t) = &tunnel_for_walk {
                        let t = t.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            t.run(peers, walker_interval).await;
                        });
                    }
                    if let Some(dht) = &dht {
                        let dht = dht.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            dht.walk_run(peers, walker_interval).await;
                        });
                    }
                    d.run(peers, walker_interval).await;
                } else {
                    tracing::warn!("aucun pair de bootstrap n'a pu etre resolu pour IPv8");
                }
            });
        }

        let stack = Arc::new(Self {
            endpoint,
            network,
            discovery,
            content_discovery,
            tunnel,
            dht,
            key,
            dht_maintenance_stop,
            anon_lanes: Mutex::new(HashMap::new()),
            engine_config: engine_config.clone(),
            downloads_dir: downloads_dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            tasks,
            circuit_bounds: Arc::new((
                AtomicUsize::new(config.min_circuits.max(1) as usize),
                AtomicUsize::new(config.max_circuits.max(1) as usize),
            )),
            socks_listen_ports: config.socks_listen_ports.clone(),
            swarm_states: Mutex::new(HashMap::new()),
            swarm_lookup: Mutex::new(HashMap::new()),
            hidden_tasks: Mutex::new(Vec::new()),
            self_weak: Mutex::new(std::sync::Weak::new()),
        });
        *stack.self_weak.lock().unwrap() = Arc::downgrade(&stack);
        Ok(stack)
    }

    /// Cle publique IPv8 (hex, affichage API).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.key.public_key().to_bin())
    }

    /// `session.overlays` : instantanes `OverlaySchema` de toutes les
    /// communities chargees, dans l'ordre de chargement Python
    /// (`DiscoveryCommunity` config, puis launchers : content, DHT,
    /// tunnel).
    pub fn overlays_info(&self) -> Vec<tribler_ipv8::OverlayInfo> {
        let mut out = vec![self.discovery.overlay_info(false)];
        if let Some(c) = &self.content_discovery {
            out.push(c.overlay_info(false));
        }
        if let Some(d) = &self.dht {
            // `DHTDiscoveryCommunity` a son propre `Network` →
            // `is_isolated` vrai (calcule dans `overlay_info`).
            out.push(d.overlay_info());
        }
        if let Some(t) = &self.tunnel {
            out.push(t.overlay_info(false));
        }
        out
    }

    /// `enable_overlay_statistics` de `OverlaysEndpoint` : active ou
    /// desactive le comptage pour tous les overlays (`all`) ou celui
    /// nomme `overlay_name` (`__class__.__name__`).
    pub async fn enable_overlay_statistics(
        &self,
        enable: bool,
        overlay_name: Option<&str>,
        all: bool,
    ) {
        for info in self.overlays_info() {
            if all || Some(info.overlay_name) == overlay_name {
                self.endpoint
                    .enable_community_statistics(info.prefix(), enable)
                    .await;
            }
        }
    }

    /// `IsolationEndpoint.add_exit_node` : `walk_to` sur les overlays
    /// tunnel (seule `TunnelCommunity` en Python).
    pub async fn walk_exit_nodes(&self, addr: &UdpAddress) {
        if let Some(t) = &self.tunnel {
            let _ = t.walk_to(addr).await;
        }
    }

    /// `IsolationEndpoint.add_bootstrap_server` : blacklist reseau
    /// (globale + chaque overlay) puis `walk_to`, et ajout de
    /// l'adresse aux `DispersyBootstrapper.ip_addresses` (chez nous :
    /// le pool de bootstrap de la discovery).
    pub async fn add_bootstrap_node(&self, addr: &UdpAddress) {
        // `session.network.blacklist.append` — discovery/content/
        // tunnel partagent `self.network` ; le DHT a le sien.
        self.network.add_blacklist(addr.clone());
        let _ = self.discovery.walk_to(addr).await;
        if let Some(c) = &self.content_discovery {
            let _ = c.walk_to(addr).await;
        }
        if let Some(d) = &self.dht {
            d.network().add_blacklist(addr.clone());
            let _ = d.walk_to(addr).await;
        }
        if let Some(t) = &self.tunnel {
            let _ = t.walk_to(addr).await;
        }
        // `bootstrapper.ip_addresses.append` (overlay Community).
        self.discovery.add_bootstrapper(addr.clone());
    }

    /// `NoBlockDHTEndpoint` : lance `DHTDiscoveryCommunity.connect_peer`
    /// en tache de fond et retourne immediatement. `false` si la
    /// community DHT n'est pas chargee (`dht is None` → 404 Python).
    pub fn connect_peer_noblock(&self, mid: [u8; 20]) -> bool {
        let Some(dht) = &self.dht else {
            return false;
        };
        let dht = dht.clone();
        tokio::spawn(async move {
            match dht.connect_peer(&mid, None).await {
                Ok(_) => tracing::debug!(mid = %hex::encode(mid), "dht connect-peer ok"),
                // `DHTError` Python : loggee, pas propagee au REST.
                Err(e) => {
                    tracing::debug!(error = %e, mid = %hex::encode(mid), "dht connect-peer echoue")
                }
            }
        });
        true
    }

    /// Moteur anonyme pour `hops` sauts (1..=3) : cree la lane a la
    /// demande — proxy SOCKS5 loopback dedie + moteur uTP-only.
    ///
    /// Erreur si l'anonymat n'est pas active ou `hops` hors bornes.
    pub async fn anon_engine(&self, hops: usize) -> Result<BtEngine> {
        if !(1..=MAX_ANON_HOPS).contains(&hops) {
            return Err(CoreError::State("anon_hops doit etre entre 1 et 3".into()));
        }
        if let Some(engine) = self
            .anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| l.engine.clone())
        {
            return Ok(engine);
        }
        let tunnel = self
            .tunnel
            .clone()
            .ok_or_else(|| CoreError::State("anonymat non active".into()))?;
        let socks = Socks5Server::new(tunnel.clone(), hops);
        // `libtorrent/socks_listen_ports[hops-1]` Tribler : port
        // configure par lane (0 = ephemere). Port configure deja pris
        // -> repli ephemere (comme `Failed to start SOCKS5 servers` +
        // poursuite cote Python, au niveau lane).
        let want_port = self.socks_listen_ports.get(hops - 1).copied().unwrap_or(0);
        let socks_addr = match socks.listen(&format!("127.0.0.1:{want_port}")).await {
            Ok(a) => a,
            Err(e) if want_port != 0 => {
                tracing::warn!(
                    error = %e,
                    hops,
                    port = want_port,
                    "port SOCKS5 anonyme configure indisponible, repli sur port ephemere"
                );
                socks
                    .listen("127.0.0.1:0")
                    .await
                    .map_err(|e2| CoreError::State(format!("socks5 bind: {e2}")))?
            }
            Err(e) => return Err(CoreError::State(format!("socks5 bind: {e}"))),
        };
        // Sockets datagramme tunnelsees (cellules `data`) : en Tribler
        // les lanes anonymes desactivent le TCP (`enable_outgoing_tcp`
        // =False) — les pairs passent en uTP a travers le tunnel, la
        // DHT et les trackers UDP aussi ; seuls les trackers HTTP(S)
        // utilisent le SOCKS5 (`http-request` one-shot).
        let udp_sockets = tribler_tunnel::tunnel_udp_socket::TunnelUdpSockets::new(
            tunnel.clone(),
            hops,
            socks_addr,
        )
        .map_err(|e| CoreError::State(format!("socket uTP tunnel: {e}")))?;
        let mut cfg = self.engine_config.clone();
        cfg.socks5_proxy = Some(format!("socks5://{socks_addr}"));
        cfg.utp_only = true;
        cfg.enable_dht = true;
        // Le bootstrap DHT tunnel ne peut aboutir qu'une fois des
        // circuits READY — attendre la readiness ici bloquerait le
        // demarrage de la lane pour rien (le bootstrap retente en
        // tache de fond).
        cfg.dht_readiness_timeout_secs = 0;
        let utp_transport = udp_sockets.utp_transport.clone();
        cfg.utp_socket = Some(udp_sockets.utp.clone());
        // Le hidden seeding recoit le uTP SYN du downloader en cellule
        // `data` e2e : la lane doit accepter les connexions entrantes
        // sur la socket tunnelsee (sans elle, librqbit_utp cache le
        // SYN faute d'accepteur puis repond RST — jamais de pair).
        // `on_e2e_finished` pyipv8 est le chemin symetrique : cote
        // seeder, le pair arrive par l'ecoute uTP du tunnel.
        cfg.utp_listen_socket = Some(udp_sockets.utp);
        cfg.dht_socket = Some(std::sync::Arc::new(udp_sockets.dht));
        cfg.udp_tracker_socket = Some(std::sync::Arc::new(udp_sockets.tracker));
        cfg.disable_lsd = true;
        cfg.listen_port = None;
        // Parite Python : le `destination`/`saveas` est global, pas de
        // sous-dossier par saut — une lane anonyme telecharge dans le
        // meme dossier par defaut que le moteur principal (sauf
        // `destination` explicite a l'ajout, qui prevaut par download).
        cfg.output_dir = self.downloads_dir.clone();
        // Fastresume par lane (meme convention que `main`) — un
        // dossier dedie empeche la restauration croisee des torrents
        // anonymes sur le moteur en clair.
        cfg.persistence_dir = Some(self.state_dir.join("rqbit").join(format!("anon{hops}")));
        let engine = BtEngine::start(cfg).await?;
        self.tasks.register(
            Some("Ipv8Stack"),
            &format!("circuit_watchdog_{hops}"),
            Some(CIRCUIT_PROBE_INTERVAL.as_secs_f64()),
        );
        let circuit_watchdog_stop = spawn_circuit_watchdog(
            tunnel.clone(),
            hops,
            engine
                .kill_switch()
                .expect("kill switch absent avec proxy configure"),
            self.circuit_bounds.clone(),
            engine.clone(),
        );
        let lane = AnonLane {
            socks_addr,
            socks,
            engine: engine.clone(),
            utp_transport,
            circuit_watchdog_stop,
        };
        self.anon_lanes.lock().unwrap().insert(hops, lane);
        // `monitor_hidden_swarms` + `on_e2e_finished` Python : le
        // suivi des swarms caches et l'injection des pairs e2e sont
        // globaux a la stack, demarres a la premiere lane.
        self.ensure_hidden_tasks(&tunnel);
        tracing::info!(hops, %socks_addr, "lane anonyme creee");
        Ok(engine)
    }

    /// Demarre (une fois) les taches hidden-services de la stack :
    /// le poll `monitor_hidden_swarms` et le relais `e2e_ready` ->
    /// pairs uTP (`on_e2e_finished` Python).
    fn ensure_hidden_tasks(&self, tunnel: &Arc<TunnelCommunity>) {
        let mut tasks = self.hidden_tasks.lock().unwrap();
        if !tasks.is_empty() {
            return;
        }
        let weak = self.self_weak.lock().unwrap().clone();
        self.tasks.register(
            Some("TriblerTunnelCommunity"),
            "monitor_hidden_swarms",
            Some(SWARM_MONITOR_INTERVAL.as_secs_f64()),
        );
        tasks.push(spawn_swarm_monitor(weak.clone(), tunnel.clone()));
        tasks.push(spawn_e2e_listener(weak, tunnel.clone()));
    }

    /// Met a jour a chaud les bornes `min_circuits`/`max_circuits`
    /// partagees avec les watchdogs de lanes (`POST /api/settings` —
    /// comme `self.settings` relu par `monitor_downloads` Python).
    pub fn set_circuit_bounds(&self, min_circuits: usize, max_circuits: usize) {
        self.circuit_bounds
            .0
            .store(min_circuits.max(1), Ordering::Relaxed);
        self.circuit_bounds
            .1
            .store(max_circuits.max(1), Ordering::Relaxed);
    }

    /// Bornes `min_circuits`/`max_circuits` effectives (pour
    /// `effective_config` / `GET /api/settings`).
    pub fn circuit_bounds(&self) -> (usize, usize) {
        (
            self.circuit_bounds.0.load(Ordering::Relaxed),
            self.circuit_bounds.1.load(Ordering::Relaxed),
        )
    }

    /// Lane anonyme pour `hops` sauts (acces interne au transport).
    fn anon_lane(&self, hops: usize) -> Option<(BtEngine, TunnelUdpSocket)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| (l.engine.clone(), l.utp_transport.clone()))
    }

    /// Liste des lanes anonymes actives (statistiques API).
    pub fn anon_lanes(&self) -> Vec<(usize, SocketAddr)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .iter()
            .map(|(h, l)| (*h, l.socks_addr))
            .collect()
    }

    /// Moteurs des lanes anonymes actives (pour `downloads()`,
    /// `pause`/`resume`/`remove` cross-moteurs).
    pub fn anon_engines(&self) -> Vec<BtEngine> {
        self.anon_lanes
            .lock()
            .unwrap()
            .values()
            .map(|l| l.engine.clone())
            .collect()
    }

    /// Arret des moteurs anonymes et de la maintenance DHT
    /// (`Session.shutdown` Python : les overlay tasks sont annulees).
    pub async fn stop(&self) {
        if let Some(tx) = &self.dht_maintenance_stop {
            let _ = tx.send(true);
        }
        for tx in self.hidden_tasks.lock().unwrap().drain(..) {
            let _ = tx.send(true);
        }
        // `exitnode_cache` Python : persiste les noeuds de sortie
        // connus (re-pinges au prochain demarrage).
        if let Some(t) = &self.tunnel {
            save_exitnode_cache(t, &self.state_dir.join(EXITNODE_CACHE_FILE));
        }
        let lanes: Vec<AnonLane> = self
            .anon_lanes
            .lock()
            .unwrap()
            .drain()
            .map(|(_, l)| l)
            .collect();
        for lane in lanes {
            let _ = lane.circuit_watchdog_stop.send(true);
            lane.engine.stop().await;
        }
    }
}

/// `load_exit_nodes` Python : lit `<pubkey_hex> <addr> <flags>` par
/// ligne. Les entrees invalides sont ignorees (cache best-effort).
fn load_exitnode_cache(path: &Path) -> Vec<(Vec<u8>, SocketAddr, i32)> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pk = hex::decode(it.next()?).ok()?;
            let addr = it.next()?.parse().ok()?;
            let flags = it.next()?.parse().ok()?;
            Some((pk, addr, flags))
        })
        .collect()
}

/// `exitnode_cache` Python (`unload` -> `Network.snapshot` filtre
/// aux exits) : les pairs verifies annoncant un flag de sortie sont
/// persistes avec leurs flags pour etre re-pinges au redemarrage.
fn save_exitnode_cache(tunnel: &TunnelCommunity, path: &Path) {
    use tribler_network_policy::exit_policy::{
        PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_HTTP, PEER_FLAG_EXIT_IPV8,
    };
    const EXIT_MASK: i32 = PEER_FLAG_EXIT_BT | PEER_FLAG_EXIT_IPV8 | PEER_FLAG_EXIT_HTTP;
    let mut out = String::new();
    for peer in tunnel.network().verified_peers() {
        let flags = tunnel.peer_flags_of(&peer.public_key_bin);
        if flags & EXIT_MASK == 0 {
            continue;
        }
        let Some(addr) = peer.address.as_ref().and_then(|a| a.to_socket_addr()) else {
            continue;
        };
        out.push_str(&format!(
            "{} {addr} {flags}\n",
            hex::encode(&peer.public_key_bin)
        ));
    }
    if out.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, out) {
        tracing::warn!(error = %e, "exitnode_cache: ecriture impossible");
    }
}

/// `monitor_hidden_swarms` de `TriblerTunnelCommunity` : poll des
/// etats de telechargement des lanes anonymes — a chaque transition
/// `join_swarm` (seeder au `Seeding`) / `leave_swarm`, et creation
/// de points d'introduction (`create_introduction_point` +
/// `establish-intro`) quand le torrent est complet.
fn spawn_swarm_monitor(
    stack: std::sync::Weak<Ipv8Stack>,
    tunnel: Arc<TunnelCommunity>,
) -> tokio::sync::watch::Sender<bool> {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWARM_MONITOR_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = rx.changed() => break,
                _ = tick.tick() => {}
            }
            let Some(stack) = stack.upgrade() else { break };
            let mut seen = std::collections::HashSet::new();
            let lanes: Vec<(usize, BtEngine)> = stack
                .anon_lanes
                .lock()
                .unwrap()
                .iter()
                .map(|(h, l)| (*h, l.engine.clone()))
                .collect();
            for (hops, engine) in lanes {
                for stat in engine.list() {
                    let Ok(raw) = hex::decode(&stat.info_hash) else {
                        continue;
                    };
                    let Ok(real_ih) = <[u8; 20]>::try_from(raw.as_slice()) else {
                        continue;
                    };
                    // `get_lookup_info_hash` : le swarm vit sous
                    // l'identite SHA1("tribler anonymous download" +
                    // hex(infohash)) — l'infohash reel reste sur le
                    // download.
                    let lookup = lookup_info_hash(&real_ih);
                    seen.insert((hops, lookup));
                    stack
                        .swarm_lookup
                        .lock()
                        .unwrap()
                        .insert(lookup, (hops, real_ih));
                    let prev = stack
                        .swarm_states
                        .lock()
                        .unwrap()
                        .insert((hops, lookup), stat.state);
                    use tribler_bittorrent::DownloadState as S;
                    if prev != Some(stat.state) {
                        match stat.state {
                            S::Downloading | S::Initializing | S::Checking => {
                                tunnel.join_swarm(lookup, hops, false);
                            }
                            S::Seeding => {
                                tunnel.join_swarm(lookup, hops, true);
                            }
                            S::Paused | S::Stopped | S::Error => {
                                tunnel.leave_swarm(&lookup);
                            }
                        }
                    }
                    // `monitor_hidden_swarms` pyipv8 : l'appel a
                    // `create_introduction_point` n'est PAS borne a la
                    // transition — il est retente a chaque tick tant
                    // que le swarm `SEEDING` n'a pas son compte de
                    // circuits IP_SEEDER (`info_hash not in ip_hashes`).
                    // Sinon un premier essai echoue (reseau encore vide
                    // de pairs exit) condamnait le hidden seeding pour
                    // toute la session.
                    if stat.state == S::Seeding {
                        ensure_introduction_points(&tunnel, lookup);
                        // Re-annonce DHT periodique (bornee par
                        // `intro_reannounce_interval`) : l'annonce
                        // faite par le point d'introduction peut etre
                        // perdue/non propagee — le seeder re-publie.
                        let t = tunnel.clone();
                        tokio::spawn(async move {
                            t.reannounce_intro_points(lookup).await;
                        });
                    }
                }
            }
            // Downloads disparus ou lanes arretees -> `leave_swarm`.
            let gone: Vec<(usize, [u8; 20])> = stack
                .swarm_states
                .lock()
                .unwrap()
                .keys()
                .filter(|k| !seen.contains(k))
                .copied()
                .collect();
            for key in gone {
                stack.swarm_states.lock().unwrap().remove(&key);
                stack.swarm_lookup.lock().unwrap().remove(&key.1);
                tunnel.leave_swarm(&key.1);
            }
        }
    });
    tx
}

/// Cree des points d'introduction jusqu'a `max_intro_points`
/// circuits `IP_SEEDER` prets/en cours pour le swarm (`new_state ==
/// SEEDING` de `monitor_hidden_swarms`).
fn ensure_introduction_points(tunnel: &Arc<TunnelCommunity>, lookup: [u8; 20]) {
    let lookup_hex = hex::encode(lookup);
    let existing = tunnel
        .circuits_info()
        .iter()
        .filter(|c| {
            c.ctype == tribler_tunnel::routing::CIRCUIT_TYPE_IP_SEEDER
                && c.info_hash.as_deref() == Some(lookup_hex.as_str())
        })
        .count();
    let target = tunnel.settings.max_intro_points;
    // `required_ip` pyipv8 : `intro_point_peer` epingle impose le
    // DERNIER saut des circuits IP_SEEDER. Pair inconnu/non verifie ->
    // `walk_to` pour le decouvrir, retry au prochain tick (creer sans
    // le pin contournerait le reglage).
    let pinned = match tunnel.settings.intro_point_peer.as_ref() {
        None => None,
        Some(addr) => match tunnel.network().get_verified_by_address(addr) {
            Some(p) => Some(p),
            None => {
                tracing::debug!(
                    addr = ?addr,
                    "intro_point_peer pas encore verifie : walk_to, retry au prochain tick"
                );
                let t = tunnel.clone();
                let a = addr.clone();
                tokio::spawn(async move {
                    let _ = t.walk_to(&a).await;
                });
                return;
            }
        },
    };
    for _ in existing..target {
        let tunnel = tunnel.clone();
        let required = pinned.clone();
        tokio::spawn(async move {
            match tunnel
                .create_introduction_point(lookup, required.as_ref())
                .await
            {
                Ok(cid) => {
                    let timeout = tunnel.settings.circuit_timeout.as_millis() as u64;
                    if tunnel.wait_circuit_ready(cid, timeout).await.is_ok() {
                        match tunnel.send_establish_intro(cid, lookup).await {
                            Ok(rx) => {
                                let _ = rx.await;
                            }
                            Err(e) => {
                                tracing::debug!(cid, error = %e, "establish-intro non envoye");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "create_introduction_point: pas de circuit IP_SEEDER");
                }
            }
        });
    }
}

/// `on_e2e_finished` Python : a la liaison d'un circuit e2e, injecte
/// le pair dans le download concerne.
///
/// - `RP_DOWNLOADER` (on telecharge) : l'adresse factice est epinglee
///   sur le circuit e2e dans la socket uTP de la lane, les cellules
///   entrantes y sont injectees et l'adresse est passee a
///   `Download::add_peer` (`dl.add_peer(circuit_id_to_ip(cid), 1024)`
///   Python — meme adresse factice pour les deux roles).
/// - `RP_SEEDER` (on seede) : l'adresse factice
///   `circuit_id_to_ip(cid):1024` est epinglee sur le circuit dans le
///   transport uTP de la lane et les cellules `data` entrantes y
///   sont injectees — les reponses repartent dans le meme circuit
///   (`set_udp_associate_default_remote` Python).
fn spawn_e2e_listener(
    stack: std::sync::Weak<Ipv8Stack>,
    tunnel: Arc<TunnelCommunity>,
) -> tokio::sync::watch::Sender<bool> {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let mut e2e = tunnel.e2e_ready();
    tokio::spawn(async move {
        loop {
            let (cid, lookup) = tokio::select! {
                _ = rx.changed() => break,
                ev = e2e.recv() => match ev {
                    Ok(v) => v,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                },
            };
            let Some(stack) = stack.upgrade() else { break };
            let Some((hops, real_ih)) = stack.swarm_lookup.lock().unwrap().get(&lookup).copied()
            else {
                continue;
            };
            let Some((engine, utp)) = stack.anon_lane(hops) else {
                continue;
            };
            let ctype = stack
                .tunnel
                .as_ref()
                .and_then(|t| {
                    t.circuits_info()
                        .into_iter()
                        .find(|c| c.circuit_id == cid)
                        .map(|c| c.ctype)
                })
                .unwrap_or_default();
            let fake = SocketAddr::V4(std::net::SocketAddrV4::new(
                circuit_id_to_ip(cid),
                CIRCUIT_ID_PORT,
            ));
            match ctype.as_str() {
                // `on_e2e_finished` pyipv8 : `add_peer` recoit
                // `circuit_id_to_ip(cid):1024` dans les DEUX roles —
                // la socket uTP de la lane est une `TunnelUdpSocket`
                // (pas d'UDP reel) : `dial` loopback serait un
                // datagramme `data` vers l'exit, jamais vu en local.
                // Le pin impose le circuit e2e lie et les cellules
                // entrantes sont injectees sous l'adresse factice.
                tribler_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER
                | tribler_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER => {
                    utp.pin_circuit(fake, cid);
                    let mut data_rx = tunnel.subscribe_circuit_data(cid);
                    let utp = utp.clone();
                    let fake2 = fake;
                    tokio::spawn(async move {
                        let mut n = 0u64;
                        while let Some(msg) = data_rx.recv().await {
                            // Le circuit e2e peut transporter autre
                            // chose que de l'uTP (mis-routage, bruit
                            // du pair) : filtrer par forme comme le
                            // fait `data_rx` de `TunnelUdpSocket`.
                            if !tribler_network_policy::exit_policy::could_be_utp(&msg.data) {
                                continue;
                            }
                            n += 1;
                            if n <= 16 || n.is_multiple_of(64) {
                                // Entete uTP : seq_nr/ack_nr (offsets 16 et
                                // 18) + 8 premiers octets de payload — pour
                                // distinguer desordre/rejeu de corruption.
                                let d = &msg.data;
                                let seq = u16::from_be_bytes([d[16], d[17]]);
                                let ack = u16::from_be_bytes([d[18], d[19]]);
                                let pay = &d[20..d.len().min(28)];
                                tracing::debug!(
                                    circuit_id = cid,
                                    n,
                                    len = d.len(),
                                    head = %hex::encode(&d[..d.len().min(8)]),
                                    seq_nr = seq,
                                    ack_nr = ack,
                                    payload_head = %hex::encode(pay),
                                    "e2e -> injection uTP"
                                );
                            }
                            utp.inject_incoming(msg.data, fake2);
                        }
                    });
                    let mut added = false;
                    if ctype == tribler_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER {
                        if let Some(dl) = engine.get_by_hash(&real_ih) {
                            added = dl.add_peer(fake);
                        }
                    }
                    tracing::info!(
                        circuit_id = cid,
                        ctype = %ctype,
                        %fake,
                        add_peer = added,
                        info_hash = %hex::encode(real_ih),
                        "e2e listener : lane branchee"
                    );
                }
                _ => {}
            }
        }
    });
    tx
}

/// Charge la cle IPv8 depuis `path` ou en cree une nouvelle (format
/// binaire `LibNaCLSK:`).
fn load_or_create_key(path: &Path) -> Result<LibNaClSecretKey> {
    match std::fs::read(path) {
        Ok(data) => LibNaClSecretKey::from_bin(&data).map_err(CoreError::from),
        Err(_) => {
            let key = LibNaClSecretKey::generate();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, key.to_bin())?;
            Ok(key)
        }
    }
}
