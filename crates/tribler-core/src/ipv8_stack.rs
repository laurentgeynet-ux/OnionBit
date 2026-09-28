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
use tribler_ipv8::peer::Network;
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::socks5::Socks5Server;
use tribler_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID;

use crate::error::{CoreError, Result};

/// Fichier de cle IPv8 persiste dans `state_dir` (`ec.pem`-equivalent,
/// format binaire `LibNaCLSK:`).
const IPV8_KEY_FILE: &str = "ipv8_keypair.bin";

/// Nombre de sauts anonymes maximum (`anon_hops` Python : 1..=3).
pub const MAX_ANON_HOPS: usize = 3;

/// Taille maximale du blob de reponse `remote_select` (borne
/// `max_response_size` Python).
const SELECT_MAX_RESPONSE: usize = 1_000_000;

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
    min_circuits: usize,
) -> Arc<tokio::sync::watch::Sender<bool>> {
    // Fail-closed des la creation de la lane (avant tout circuit).
    ks.engage_scoped("circuits", format!("aucun circuit READY a {hops} sauts"));
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let mut changes = tunnel.watch_circuits();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CIRCUIT_PROBE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Tentative immediate de construction de circuit(s). Plusieurs
        // circuits en parallele (`min_circuits`, `TunnelSettings`
        // pyipv8) sont necessaires : un unique circuit dont le
        // dernier saut annonce `EXIT_HTTP` sans le servir reellement
        // (pair instable/mensonger du reseau public) bloque
        // definitivement la lane sinon, faute d'alternative pour le
        // selecteur SOCKS5 (`select_http_circuit`).
        let _ = tunnel.build_circuits_if_needed(hops, min_circuits).await;
        // Premier tick immediat : absorbe (l'evaluation initiale est
        // deja faite ci-dessus).
        tick.tick().await;
        loop {
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {
                    let _ = tunnel.build_circuits_if_needed(hops, min_circuits).await;
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
}

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

    /// Select distant : parametres JSON (`txt_filter`, `infohash_set`,
    /// `first`/`last`) → entrees `.mdblob` compressees LZ4
    /// (`entries_to_chunk` + `lz4.frame` cote Python).
    fn remote_select(&self, json: &[u8]) -> Vec<u8> {
        let query: serde_json::Value = serde_json::from_slice(json).unwrap_or_default();
        let rows = self
            .db
            .with(|conn| select_rows(conn, &query))
            .unwrap_or_default();
        let mut blob = Vec::new();
        for row in rows {
            if let Some(entry) = Self::row_to_entry(&row) {
                if let Ok(bytes) = tribler_format::mdblob::encode_entry_presigned(&entry) {
                    if blob.len() + bytes.len() > SELECT_MAX_RESPONSE {
                        break;
                    }
                    blob.extend_from_slice(&bytes);
                }
            }
        }
        {
            use std::io::Write;
            let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
            if enc.write_all(&blob).is_err() {
                return Vec::new();
            }
            enc.finish().unwrap_or_default()
        }
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
        self.db
            .with(|conn| {
                let mut results = Vec::new();
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
                }
                Ok(results)
            })
            .unwrap_or_default()
    }

    /// `(version, plateforme)` pour `VersionResponse`.
    fn version_info(&self) -> (String, String) {
        (env!("CARGO_PKG_VERSION").to_string(), "Tribler Rust".into())
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

/// Filtre les lignes `channel_node` selon la requete JSON distante
/// (`sanitize_query` Python : `infohash_set`, `txt_filter`, `first`,
/// `last`).
fn select_rows(
    conn: &rusqlite::Connection,
    query: &serde_json::Value,
) -> tribler_db::Result<Vec<tribler_db::models::ChannelNodeRow>> {
    let first = query.get("first").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let last = query
        .get("last")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let txt = query
        .get("txt_filter")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut rows = if !txt.is_empty() {
        tribler_db::channel::search_by_title(conn, &format!("%{txt}%"), 200)?
    } else if let Some(set) = query.get("infohash_set").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for v in set {
            if let Some(hex) = v.as_str() {
                if let Ok(bytes) = hex::decode(hex.trim_matches('"')) {
                    if let Some(row) = tribler_db::channel::get_by_infohash(conn, &bytes)? {
                        out.push(row);
                    }
                }
            }
        }
        out
    } else {
        // Pas de filtre : entrees torrent les plus recentes.
        tribler_db::channel::search_by_title(conn, "%", 200)?
    };
    let sliced: Vec<_> = rows.drain(first.min(rows.len())..).collect();
    let mut sliced = sliced;
    if let Some(l) = last {
        sliced.truncate(l.saturating_sub(first) + 1);
    }
    Ok(sliced)
}

/// Stack IPv8 de session (endpoint + communities + lanes anonymes).
pub struct Ipv8Stack {
    /// Endpoint UDP IPv8 partage.
    pub endpoint: Arc<UdpEndpoint>,
    /// Registre de pairs IPv8.
    pub network: Arc<Network>,
    /// Community de decouverte de pairs.
    pub discovery: Arc<DiscoveryCommunity>,
    /// Community de decouverte de contenu.
    pub content_discovery: Arc<ContentDiscoveryCommunity>,
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
    /// Registre partage des taches nommees (`/api/ipv8/asyncio/tasks`).
    tasks: crate::asyncio::TaskRegistry,
    /// `TunnelSettings.min_circuits` (cf. `Ipv8Config::min_circuits`).
    min_circuits: usize,
}

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
        let endpoint = match UdpEndpoint::bind(&config.listen_addr).await {
            Ok(ep) => ep,
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
        let provider = Arc::new(SessionContentProvider { db });
        let content_discovery = ContentDiscoveryCommunity::new(
            key.clone(),
            network.clone(),
            endpoint.clone(),
            provider,
            None,
        )
        .await;
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
            Some(
                TunnelCommunity::new_with_id(
                    key.clone(),
                    network.clone(),
                    endpoint.clone(),
                    config.peer_flags,
                    community_id,
                )
                .await,
            )
        } else {
            None
        };

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
            let mut v = vec![
                tribler_ipv8::prefix_of(&tribler_ipv8::discovery::DISCOVERY_COMMUNITY_ID),
                tribler_ipv8::prefix_of(&tribler_ipv8::CONTENT_DISCOVERY_COMMUNITY_ID),
            ];
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

        // Bootstrap discovery en tache de fond (resolution DNS + IP).
        // `register_anonymous_task("bootstrap", ...)` Python.
        let bootstrap_peers_config = config.bootstrap_peers.clone();
        if !bootstrap_peers_config.is_empty() {
            tasks.register(None, "bootstrap", None);
            let d = discovery.clone();
            let dht = dht.clone();
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
                    d.run(peers).await;
                } else {
                    tracing::warn!("aucun pair de bootstrap n'a pu etre resolu pour IPv8");
                }
            });
        }

        Ok(Arc::new(Self {
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
            tasks,
            min_circuits: config.min_circuits.max(1) as usize,
        }))
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
        let mut out = vec![
            self.discovery.overlay_info(false),
            self.content_discovery.overlay_info(false),
        ];
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
        let _ = self.content_discovery.walk_to(addr).await;
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
        let socks_addr = socks
            .listen("127.0.0.1:0")
            .await
            .map_err(|e| CoreError::State(format!("socks5 bind: {e}")))?;
        let mut cfg = self.engine_config.clone();
        cfg.socks5_proxy = Some(format!("socks5://{socks_addr}"));
        cfg.utp_only = true;
        cfg.enable_dht = false;
        cfg.disable_lsd = true;
        cfg.listen_port = None;
        cfg.output_dir = self.downloads_dir.join(format!("anon{hops}"));
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
            self.min_circuits,
        );
        let lane = AnonLane {
            socks_addr,
            socks,
            engine: engine.clone(),
            circuit_watchdog_stop,
        };
        self.anon_lanes.lock().unwrap().insert(hops, lane);
        tracing::info!(hops, %socks_addr, "lane anonyme creee");
        Ok(engine)
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
