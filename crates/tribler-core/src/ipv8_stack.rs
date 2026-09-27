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
) -> Arc<tokio::sync::watch::Sender<bool>> {
    // Fail-closed des la creation de la lane (avant tout circuit).
    ks.engage_scoped("circuits", format!("aucun circuit READY a {hops} sauts"));
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let mut changes = tunnel.watch_circuits();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CIRCUIT_PROBE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Premier tick immediat : absorbe (l'evaluation initiale est
        // deja faite ci-dessus).
        tick.tick().await;
        loop {
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {}
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
    /// entrees et les insere dans `channel_node`.
    fn process_select_response(&self, blob: &[u8]) {
        use std::io::Read;
        let mut data = Vec::new();
        if lz4_flex::frame::FrameDecoder::new(blob)
            .read_to_end(&mut data)
            .is_err()
        {
            return;
        }
        let Ok(entries) = tribler_format::mdblob::parse_blob(&data) else {
            return;
        };
        let _ = self.db.with(|conn| {
            for e in &entries {
                if let Some(row) = entry_to_row(e) {
                    let _ = tribler_db::channel::insert(conn, &row);
                }
            }
            Ok(())
        });
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
    /// Cle IPv8 de la session (persistee dans `state_dir`).
    key: LibNaClSecretKey,
    /// Moteurs anonymes par nombre de sauts (1..=3).
    anon_lanes: Mutex<HashMap<usize, AnonLane>>,
    /// Config moteur de base (pour creer les lanes anonymes).
    engine_config: EngineConfig,
    /// Repertoire de telechargement par defaut.
    downloads_dir: PathBuf,
}

impl Ipv8Stack {
    /// Cree et demarre la stack : endpoint, communities, discovery
    /// bootstrap (tache de fond).
    pub async fn start(
        config: &Ipv8Config,
        state_dir: &Path,
        downloads_dir: &Path,
        engine_config: &EngineConfig,
        db: Arc<Database>,
    ) -> Result<Arc<Self>> {
        let key = load_or_create_key(&state_dir.join(IPV8_KEY_FILE))?;
        let endpoint = UdpEndpoint::bind(&config.listen_addr)
            .await
            .map_err(|e| CoreError::State(format!("bind ipv8: {e}")))?;
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

        // Tache de reception de l'endpoint (dispatch prefixe).
        let ep = endpoint.clone();
        tokio::spawn(async move {
            if let Err(e) = ep.run().await {
                tracing::error!(error = %e, "endpoint ipv8 termine");
            }
        });

        // Bootstrap discovery en tache de fond.
        let peers: Vec<UdpAddress> = config
            .bootstrap_peers
            .iter()
            .filter_map(|p| p.parse::<SocketAddr>().ok())
            .map(UdpAddress::from)
            .collect();
        if !peers.is_empty() {
            let d = discovery.clone();
            tokio::spawn(async move {
                d.run(peers).await;
            });
        }

        Ok(Arc::new(Self {
            endpoint,
            network,
            discovery,
            content_discovery,
            tunnel,
            key,
            anon_lanes: Mutex::new(HashMap::new()),
            engine_config: engine_config.clone(),
            downloads_dir: downloads_dir.to_path_buf(),
        }))
    }

    /// Cle publique IPv8 (hex, affichage API).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.key.public_key().to_bin())
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
        cfg.output_dir = self.downloads_dir.join(format!("anon{hops}"));
        let engine = BtEngine::start(cfg).await?;
        let circuit_watchdog_stop = spawn_circuit_watchdog(
            tunnel.clone(),
            hops,
            engine
                .kill_switch()
                .expect("kill switch absent avec proxy configure"),
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

    /// Arret des moteurs anonymes.
    pub async fn stop(&self) {
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
