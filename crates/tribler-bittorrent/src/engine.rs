//! Moteur BitTorrent : enveloppe de `librqbit::Session`.
//!
//! `BtEngine` est le port "infrastructure" du domaine : il gere le
//! cycle de vie de la session (creation, arret) et l'ajout/suppression/
//! pause/reprise des telechargements. Il ne connait ni REST ni base de
//! donnees : ces couches vivent dans `tribler-api` et `tribler-db`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use librqbit::api::TorrentIdOrHash;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse};
use tribler_network_policy::kill_switch::KillSwitch;
use tribler_network_policy::proxy_guard::validate_local_socks5_url;

use crate::config::EngineConfig;
use crate::download::{Download, DownloadStats};
use crate::error::{BtError, Result};

/// Intervalle entre deux sondes TCP du proxy SOCKS5 (kill switch).
const PROXY_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// Timeout d'une sonde TCP du proxy SOCKS5.
const PROXY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Moteur BitTorrent du daemon (equivalent de
/// `tribler.core.libtorrent.download_manager.DownloadManager`).
///
/// Clone leger (`Arc` interne).
#[derive(Clone)]
pub struct BtEngine {
    session: Arc<librqbit::Session>,
    config: EngineConfig,
    /// Kill switch d'anonymat : present uniquement quand
    /// `socks5_proxy` est configure. Sonde le proxy et suspend tout
    /// trafic pair si l'anonymat n'est plus garanti.
    kill_switch: Option<Arc<KillSwitch>>,
    /// Arret du watchdog de sondage du proxy.
    watchdog_stop: Option<Arc<tokio::sync::watch::Sender<bool>>>,
    /// Trackers ajoutes a chaud par info-hash (rqbit ne permet pas de
    /// muter `shared.trackers` apres initialisation). Un `Arc` par
    /// torrent, partage entre tous les clones `Download`.
    extra_trackers: ExtraTrackers,
}

/// Liste de trackers additionnels d'un torrent, partagee entre les
/// clones `Download` (`PUT /downloads/{ih}/trackers` Python).
pub type TrackerList = Arc<std::sync::Mutex<Vec<String>>>;

/// Table info-hash -> trackers additionnels (un `Arc` par torrent).
type ExtraTrackers = Arc<std::sync::Mutex<std::collections::HashMap<[u8; 20], TrackerList>>>;

impl std::fmt::Debug for BtEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BtEngine")
            .field("output_dir", &self.config.output_dir)
            .field("dht", &self.config.enable_dht)
            .finish_non_exhaustive()
    }
}

impl BtEngine {
    /// Demarre une session BitTorrent.
    ///
    /// Cree le repertoire de sortie si absent. En config `offline`
    /// (tests), aucun trafic reseau n'est emis.
    pub async fn start(config: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.output_dir)?;
        // Proxy guard : le SOCKS5 doit etre le proxy local des tunnels
        // — une URL distante fait echouer le demarrage (pas de repli
        // silencieux vers une connexion directe).
        let proxy_addr = config
            .socks5_proxy
            .as_deref()
            .map(validate_local_socks5_url)
            .transpose()?;
        let session = librqbit::Session::new_with_opts(
            config.output_dir.clone(),
            config.to_session_options(),
        )
        .await
        .map_err(|e| BtError::Engine(e.to_string()))?;
        // `libtorrent/dht_readiness_timeout` Tribler : attendre que la
        // DHT soit peuplee avant de declarer la session prete (sans
        // effet si la DHT est desactivee).
        if config.enable_dht && config.dht_readiness_timeout_secs > 0 {
            Self::wait_dht_ready(
                &session,
                Duration::from_secs(config.dht_readiness_timeout_secs),
            )
            .await;
        }
        let (kill_switch, watchdog_stop) = match proxy_addr {
            Some(addr) => {
                let ks = Arc::new(KillSwitch::new());
                let stop = Self::spawn_proxy_watchdog(addr, ks.clone());
                (Some(ks), Some(stop))
            }
            None => (None, None),
        };
        tracing::info!(
            output_dir = %config.output_dir.display(),
            dht = config.enable_dht,
            listen = ?config.listen_port,
            proxy = ?proxy_addr,
            "session bittorrent demarree"
        );
        Ok(Self {
            session,
            config,
            kill_switch,
            watchdog_stop,
            extra_trackers: Default::default(),
        })
    }

    /// Kill switch d'anonymat partage (ex. pour que `tribler-tunnel`
    /// l'engage quand tous les circuits sont morts). `None` si aucun
    /// proxy anonyme n'est configure.
    pub fn kill_switch(&self) -> Option<Arc<KillSwitch>> {
        self.kill_switch.clone()
    }

    /// Watchdog du proxy : sonde TCP periodique ; engage le kill
    /// switch tant que le proxy est injoignable (le trafic pair
    /// anonyme est suspendu par [`KillSwitch::guard`]).
    fn spawn_proxy_watchdog(
        addr: SocketAddr,
        ks: Arc<KillSwitch>,
    ) -> Arc<tokio::sync::watch::Sender<bool>> {
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PROXY_PROBE_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = tick.tick() => {
                        let probe = tokio::time::timeout(
                            PROXY_PROBE_TIMEOUT,
                            tokio::net::TcpStream::connect(addr),
                        )
                        .await;
                        match probe {
                            Ok(Ok(_)) => {
                                if ks.release_scoped("proxy") {
                                    tracing::info!(%addr, "proxy SOCKS5 de nouveau joignable");
                                }
                            }
                            _ => ks.engage_scoped(
                                "proxy",
                                format!("proxy SOCKS5 {addr} injoignable"),
                            ),
                        }
                    }
                }
            }
        });
        Arc::new(stop_tx)
    }

    /// Arret propre de la session et de toutes ses taches.
    pub async fn stop(&self) {
        if let Some(tx) = &self.watchdog_stop {
            let _ = tx.send(true);
        }
        if self.config.clear_orphaned_parts {
            self.remove_orphaned_parts();
        }
        self.session.stop().await;
        tracing::info!("session bittorrent arretee");
    }

    /// `rm_orphaned_files_and_subfolders` Python, restreint aux
    /// fichiers `*.parts` (comme `clear_orphaned_parts`) : un `.parts`
    /// `<infohash>.parts` (nommage libtorrent) est orphelin quand son
    /// info-hash n'est pas parmi les telechargements connus du moteur.
    /// librqbit n'ecrit pas de `.parts` lui-meme — le nettoyage couvre
    /// les restes laisses par d'autres clients dans le meme dossier.
    fn remove_orphaned_parts(&self) {
        let known: std::collections::HashSet<String> = self
            .session
            .with_torrents(|it| it.map(|(_, t)| hex::encode(t.info_hash().0)).collect());
        let Ok(entries) = std::fs::read_dir(&self.config.output_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("parts") {
                continue;
            }
            let orphan = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|stem| !known.contains(stem))
                .unwrap_or(true);
            if orphan {
                match std::fs::remove_file(&path) {
                    Ok(()) => tracing::info!(path = %path.display(), ".parts orphelin supprime"),
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), ".parts orphelin non supprime")
                    }
                }
            }
        }
    }

    /// Attend que la table de routage DHT soit peuplee (borne par
    /// `timeout`) — `dht_readiness_timeout` Python : Tribler reporte
    /// les premieres annonces tant que la DHT n'a pas de noeuds.
    async fn wait_dht_ready(session: &librqbit::Session, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        let poll = Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            let ready = session.get_dht().is_some_and(|dht| {
                let stats = dht.stats();
                stats.routing_table_size + stats.routing_table_size_v6 > 0
            });
            if ready {
                tracing::info!("dht prete");
                return;
            }
            tokio::time::sleep(poll).await;
        }
        tracing::warn!(
            timeout_secs = timeout.as_secs(),
            "dht_readiness_timeout ecoule — session prete sans DHT peuplee"
        );
    }

    /// Configuration en cours.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Port TCP/uTP d'ecoute effectif (si l'ecoute est activee).
    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.session.listen_addr()
    }

    /// Ajoute un telechargement depuis une URI `magnet:` ou `http(s):`
    /// (le `.torrent` est telecharge par `librqbit`).
    ///
    /// Pour `list_only` (enumeration des fichiers sans telecharger),
    /// passer par [`Self::add_with_options`].
    pub async fn add_uri(&self, uri: &str) -> Result<Download> {
        self.add(AddTorrent::from_url(uri), None).await
    }

    /// Ajoute un telechargement depuis les octets d'un fichier
    /// `.torrent` (deja borne/valide en amont par `tribler-format`).
    pub async fn add_torrent_bytes(
        &self,
        bytes: impl Into<bytes::Bytes>,
        paused: bool,
    ) -> Result<Download> {
        let opts = AddTorrentOptions {
            paused,
            overwrite: true,
            ..Default::default()
        };
        self.add(AddTorrent::from_bytes(bytes), Some(opts)).await
    }

    /// Ajoute un telechargement avec options completes.
    pub async fn add_with_options(
        &self,
        add: AddTorrent<'_>,
        opts: Option<AddTorrentOptions>,
    ) -> Result<Download> {
        self.add(add, opts).await
    }

    /// Ajoute une URI avec des reglages par telechargement (equivalent
    /// du `DownloadConfig` applique par `start_download` Python).
    pub async fn add_uri_opts(
        &self,
        uri: &str,
        opts: &crate::add_options::AddDownloadOptions,
    ) -> Result<Download> {
        self.add(AddTorrent::from_url(uri), Some(rqbit_opts(opts)))
            .await
    }

    /// Ajoute des octets `.torrent` avec des reglages par
    /// telechargement.
    pub async fn add_torrent_bytes_opts(
        &self,
        bytes: impl Into<bytes::Bytes>,
        opts: &crate::add_options::AddDownloadOptions,
    ) -> Result<Download> {
        self.add(AddTorrent::from_bytes(bytes), Some(rqbit_opts(opts)))
            .await
    }

    async fn add(&self, add: AddTorrent<'_>, opts: Option<AddTorrentOptions>) -> Result<Download> {
        if let Some(ks) = &self.kill_switch {
            ks.guard_add()?;
        }
        let response = self
            .session
            .add_torrent(add, opts)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))?;
        match response {
            AddTorrentResponse::AlreadyManaged(_, handle)
            | AddTorrentResponse::Added(_, handle) => Ok(self.wrap(handle)),
            AddTorrentResponse::ListOnly(_) => Err(BtError::NoHandle),
        }
    }

    /// Enveloppe un handle rqbit en `Download` (avec la liste de
    /// trackers additionnels partagee du moteur, keyed par info-hash).
    fn wrap(&self, inner: Arc<librqbit::ManagedTorrent>) -> Download {
        let key = inner.info_hash().0;
        let extra_trackers = self
            .extra_trackers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key)
            .or_default()
            .clone();
        Download {
            inner,
            extra_trackers,
        }
    }

    /// Liste les telechargements courants (instantanes d'etat).
    pub fn list(&self) -> Vec<DownloadStats> {
        self.session.with_torrents(|it| {
            it.map(|(_, t)| self.wrap(Arc::clone(t)))
                .map(|d| d.stats())
                .collect()
        })
    }

    /// Recupere un telechargement par id interne ou info-hash hex.
    ///
    /// Attention : une chaine **numerique** est un id interne — pour la
    /// couche API (lookup strict par info-hash comme `unhexlify`
    /// Python), preferer [`Self::get_by_hash`].
    pub fn get(&self, id_or_hash: &str) -> Option<Download> {
        let key = parse_id_or_hash(id_or_hash)?;
        self.session.get(key).map(|inner| self.wrap(inner))
    }

    /// Recupere un telechargement par info-hash **v1 brut** (20 octets)
    /// — lookup strict equivalent a `unhexlify(match_info["infohash"])`
    /// Python : jamais d'interpretation numerique (un infohash
    /// `"00..0"` tout-chiffres serait sinon pris pour l'id 0).
    pub fn get_by_hash(&self, infohash: &[u8]) -> Option<Download> {
        let id: [u8; 20] = infohash.try_into().ok()?;
        self.session
            .get(TorrentIdOrHash::Hash(librqbit_core::hash_id::Id20::new(id)))
            .map(|inner| self.wrap(inner))
    }

    /// Met en pause un telechargement.
    pub async fn pause(&self, id_or_hash: &str) -> Result<()> {
        let d = self
            .get(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        self.session
            .pause(&d.inner)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
    }

    /// Reprend un telechargement en pause.
    pub async fn resume(&self, id_or_hash: &str) -> Result<()> {
        if let Some(ks) = &self.kill_switch {
            ks.guard()?;
        }
        let d = self
            .get(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        self.session
            .unpause(&d.inner)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
    }

    /// Met en pause tous les telechargements (suspension mobile /
    /// arret rapide). Les pauses unitaires qui echouent sont
    /// collectees, pas fatales.
    pub async fn pause_all(&self) -> Vec<String> {
        let ids: Vec<String> = self
            .session
            .with_torrents(|it| it.map(|(id, _)| id.to_string()).collect());
        let mut errors = Vec::new();
        for id in ids {
            if let Err(e) = self.pause(&id).await {
                errors.push(format!("{id}: {e}"));
            }
        }
        errors
    }

    /// Reprend tous les telechargements en pause. Respecte le kill
    /// switch : engage, aucune reprise n'est tente et la raison est
    /// retournee comme erreur unique.
    pub async fn resume_all(&self) -> Vec<String> {
        if let Some(ks) = &self.kill_switch {
            if let Err(e) = ks.guard() {
                return vec![e.to_string()];
            }
        }
        let ids: Vec<String> = self
            .session
            .with_torrents(|it| it.map(|(id, _)| id.to_string()).collect());
        let mut errors = Vec::new();
        for id in ids {
            if let Err(e) = self.resume(&id).await {
                errors.push(format!("{id}: {e}"));
            }
        }
        errors
    }

    /// Supprime un telechargement de la session.
    ///
    /// `delete_files` efface aussi les donnees sur disque — ne doit etre
    /// vrai que sur demande explicite de l'utilisateur (couche API).
    pub async fn remove(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let key = parse_id_or_hash(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        self.session
            .delete(key, delete_files)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
    }

    /// Restreint les fichiers telecharges (`selected_files` de
    /// `PATCH /downloads/{ih}` — `Session::update_only_files` rqbit,
    /// valable aussi bien sur un torrent en pause que live).
    pub async fn update_only_files(
        &self,
        id_or_hash: &str,
        only_files: &std::collections::HashSet<usize>,
    ) -> Result<()> {
        let d = self
            .get(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        self.session
            .update_only_files(&d.inner, only_files)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
    }

    /// Limites de debit de la session entiere (octets/s, `None` =
    /// illimite) — `Session::ratelimits` rqbit, modifiable a chaud
    /// (`libtorrent/max_upload_rate`/`max_download_rate` Tribler).
    pub fn set_ratelimits(&self, upload_bps: Option<u64>, download_bps: Option<u64>) {
        let to_nz = |v: Option<u64>| {
            v.and_then(|v| u32::try_from(v).ok())
                .and_then(std::num::NonZeroU32::new)
        };
        self.session.ratelimits.set_upload_bps(to_nz(upload_bps));
        self.session
            .ratelimits
            .set_download_bps(to_nz(download_bps));
    }

    /// Limites de session courantes (octets/s).
    pub fn ratelimits(&self) -> (Option<u64>, Option<u64>) {
        (
            self.session
                .ratelimits
                .get_upload_bps()
                .map(|v| v.get() as u64),
            self.session
                .ratelimits
                .get_download_bps()
                .map(|v| v.get() as u64),
        )
    }

    /// Re-annonce aux trackers : librqbit recree ses boucles
    /// d'annonce au `unpause` — pause + reprise declenche une annonce
    /// complete (superset du `force_reannounce(url)` Python, qui ne
    /// vise qu'un tracker ; l'ecart est documente dans le mapping).
    ///
    /// Torrent en pause : no-op — l'annonce repartira au resume,
    /// comme un `force_reannounce` sur un handle inactif en Python.
    pub async fn force_announce(&self, id_or_hash: &str) -> Result<()> {
        let d = self
            .get(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        if d.is_paused() {
            return Ok(());
        }
        self.session
            .pause(&d.inner)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))?;
        self.session
            .unpause(&d.inner)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
    }

    /// Bitfield des pieces detenues, base64 MSB-first (meme encodage
    /// que `download.get_pieces_base64()` Python). `None` tant que le
    /// torrent n'est pas dans un etat live (equivalent du `""` Python
    /// sans handle — l'appelant traduit).
    pub fn have_pieces_base64(&self, id_or_hash: &str) -> Option<String> {
        let key = parse_id_or_hash(id_or_hash)?;
        let api = librqbit::Api::new(self.session.clone(), None);
        let (bf, _len) = api.api_dump_haves(key).ok()?;
        use base64::Engine as _;
        Some(base64::engine::general_purpose::STANDARD.encode(bf.as_raw_slice()))
    }

    /// Stats par pair du torrent (snapshot `per_peer_stats` rqbit) —
    /// sous-ensemble des champs `peers` de `GET /api/downloads` Python.
    /// `None` si le torrent n'est pas live (pas de connexions).
    pub fn peer_stats(&self, id_or_hash: &str) -> Option<Vec<DownloadPeer>> {
        let d = self.get(id_or_hash)?;
        let live = d.inner.live()?;
        let snap = live.per_peer_stats_snapshot(Default::default());
        Some(
            snap.peers
                .iter()
                .filter_map(|(addr, p)| {
                    let addr: SocketAddr = addr.parse().ok()?;
                    Some(DownloadPeer {
                        ip: addr.ip().to_string(),
                        port: addr.port(),
                        state: p.state,
                        connection_kind: p.conn_kind.map(|k| k.to_string()),
                        client_name: p.client_name.clone(),
                        uploaded_bytes: p.counters.uploaded_bytes,
                        downloaded_bytes: p.counters.fetched_bytes,
                        incoming: p.counters.incoming_connections > 0,
                    })
                })
                .collect(),
        )
    }
}

/// Traduit les reglages par telechargement en `AddTorrentOptions`
/// rqbit (`overwrite` force pour reprendre l'existant, comme au
/// chargement Python des checkpoints).
fn rqbit_opts(o: &crate::add_options::AddDownloadOptions) -> AddTorrentOptions {
    let to_nz = |v: Option<u64>| {
        v.and_then(|v| u32::try_from(v).ok())
            .and_then(std::num::NonZeroU32::new)
    };
    AddTorrentOptions {
        paused: o.paused,
        overwrite: true,
        output_folder: o.output_folder.as_ref().map(|p| p.display().to_string()),
        only_files: o.only_files.clone(),
        trackers: (!o.trackers.is_empty()).then(|| o.trackers.clone()),
        ratelimits: librqbit::limits::LimitsConfig {
            upload_bps: to_nz(o.upload_limit_bps),
            download_bps: to_nz(o.download_limit_bps),
        },
        ..Default::default()
    }
}

/// Stats d'un pair connecte (sous-ensemble du dict `peers` Python —
/// les champs que librqbit n'expose pas restent aux defauts cote API).
#[derive(Debug, Clone)]
pub struct DownloadPeer {
    /// IP du pair.
    pub ip: String,
    /// Port du pair.
    pub port: u16,
    /// Etat interne de la connexion (`live`, …).
    pub state: &'static str,
    /// Transport (`Tcp`/`Utp`/`Socks`).
    pub connection_kind: Option<String>,
    /// Nom du client distant si connu.
    pub client_name: Option<String>,
    /// Octets envoyes a ce pair.
    pub uploaded_bytes: u64,
    /// Octets recus de ce pair.
    pub downloaded_bytes: u64,
    /// Connexion entrante (initiee par le pair).
    pub incoming: bool,
}

/// Parse `id_or_hash` : info-hash hex 40 caracteres en priorite (une
/// chaine tout-chiffres de 40c est un info-hash legitime — la lire
/// comme un id interne resoudrait le mauvais torrent, ex. `"00..0"`),
/// puis id numerique interne (`usize`) en repli.
fn parse_id_or_hash(s: &str) -> Option<TorrentIdOrHash> {
    if s.len() == 40 {
        if let Ok(bytes) = hex::decode(s) {
            let mut id = [0u8; 20];
            id.copy_from_slice(&bytes);
            return Some(TorrentIdOrHash::Hash(librqbit_core::hash_id::Id20::new(id)));
        }
    }
    if let Ok(id) = s.parse::<usize>() {
        return Some(TorrentIdOrHash::Id(id));
    }
    None
}
