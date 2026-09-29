//! `CoreSession` : orchestration du domaine.
//!
//! Equivalent de `tribler.core.session.Session` : assemble les ports
//! d'infrastructure (moteur BitTorrent, base) derriere une facade
//! coherente, publie les evenements sur le [`Notifier`], et restaure
//! les telechargements connus au demarrage.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tribler_bittorrent::{AddDownloadOptions, BtEngine, Download, DownloadState, DownloadStats};
use tribler_db::{Database, DownloadRow};

use crate::config::CoreConfig;
use crate::error::{CoreError, Result};
use crate::notifier::{Notification, Notifier};

/// Seuil d'espace libre declenchant `low_space` (1 Gio) — le topic
/// n'est plus emis par Tribler 8.x ; cette sonde d'ajout le re-active
/// cote daemon (format `disk_usage_data` identique).
const LOW_SPACE_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024;

/// Unite de temps des timestamps persistants : secondes Unix.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Sens de deplacement dans la file d'attente (`queue_position` du
/// `PATCH /api/downloads/{ih}` Python).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueOp {
    /// `queue_up` : remonte d'une position.
    Up,
    /// `queue_top` : tout en haut.
    Top,
    /// `queue_down` : descend d'une position.
    Down,
    /// `queue_bottom` : tout en bas.
    Bottom,
}

/// Session coeur du daemon.
///
/// `Clone` : partageable entre les handlers API et les services.
#[derive(Clone)]
pub struct CoreSession {
    inner: Arc<Inner>,
}

/// Handles des services secondaires (tous optionnels, actives par
/// la configuration — etape 14).
#[derive(Default)]
struct Services {
    watch_folder: Option<crate::services::watch_folder::WatchFolderService>,
    rss: Option<Arc<crate::services::rss::RssManager>>,
    checker: Option<Arc<crate::services::torrent_checker::TorrentChecker>>,
    /// Arret de la boucle periodique du checker.
    checker_stop: Option<tokio::sync::watch::Sender<bool>>,
}

/// Reglages applicables a chaud sans redemarrer la session.
#[derive(Default)]
struct ServiceOverrides {
    /// Flux RSS surveilles (`Some` = remplace `config.rss_urls`).
    rss_urls: Option<Vec<String>>,
    /// Repertoire surveille (`Some` = remplace `config.watch_folder_dir`).
    watch_folder_dir: Option<Option<std::path::PathBuf>>,
    /// Dossier de telechargement par defaut (`Some` = remplace `config.engine.output_dir`).
    download_dir: Option<std::path::PathBuf>,
    /// Bornes de la file (`Some` = remplace `config.queue`) —
    /// `set_session_limits` Python applique aussi les `active_*` a
    /// chaud.
    queue: Option<crate::config::QueueLimits>,
    /// Limites de debit de session (`Some` = remplace
    /// `config.engine.max_*_bps`) — appliquees a chaud via
    /// `Session::ratelimits` rqbit.
    rate_limits: Option<(Option<u64>, Option<u64>)>,
}

/// Parametres initiaux communs de `persist`/`persist_torrent`.
struct PersistParams {
    paused: bool,
    anon_hops: u32,
    safe_seeding: bool,
    extra_trackers: Vec<String>,
}

struct Inner {
    config: CoreConfig,
    /// Sous-ensemble de reglages mutables a chaud (`POST /api/settings`) :
    /// superposes a `config` par `effective_config()`.
    overrides: std::sync::RwLock<ServiceOverrides>,
    engine: BtEngine,
    db: Arc<Database>,
    notifier: Notifier,
    services: std::sync::Mutex<Services>,
    /// Stack IPv8 (discovery, content discovery, tunnel, lanes
    /// anonymes) — `Some` si `config.ipv8.enabled`.
    ipv8: Option<Arc<crate::ipv8_stack::Ipv8Stack>>,
    /// Dernier fetch de `trackers_file_sync_url`
    /// (`Download.LAST_TRACKER_FILE_SYNC` Python — TTL
    /// [`crate::trackers::TRACKER_SYNC_TTL_SECS`]).
    last_tracker_sync: std::sync::Mutex<Option<std::time::Instant>>,
    /// Etat de `/api/ipv8/asyncio/*` : derive des ticks, registre des
    /// taches nommees (`all_tasks()`), buffer du debug log.
    asyncio: crate::asyncio::AsyncioMonitor,
    /// `stop()` idempotent : plusieurs sources peuvent demander
    /// l'arret (Ctrl-C, item « Quitter » du systray, `PUT
    /// /api/shutdown` puis le graceful shutdown du serveur).
    stopped: std::sync::atomic::AtomicBool,
    /// Fin de la restauration des telechargements persistes
    /// (`load_checkpoint` Python — asynchrone) : `false` → `true`
    /// quand la tache de fond a termine ; [`Self::wait_restored`]
    /// permet aux tests de l'attendre.
    restore_done: tokio::sync::watch::Sender<bool>,
}

impl std::fmt::Debug for CoreSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreSession")
            .field("state_dir", &self.inner.config.state_dir)
            .finish_non_exhaustive()
    }
}

impl CoreSession {
    /// Demarre la session : ouvre la base, cree le moteur BitTorrent,
    /// lance la boucle de publication de progression. La restauration
    /// des telechargements persistes (`load_checkpoint` Python) part
    /// en tache de fond — la verification initiale des pieces peut
    /// prendre plusieurs secondes par torrent et ne doit pas retarder
    /// le bind de l'API de controle. Les downloads restaurés
    /// apparaissent progressivement cote clients ; [`Self::wait_restored`]
    /// attend la fin si besoin.
    pub async fn start(config: CoreConfig, notifier: Notifier) -> Result<Self> {
        std::fs::create_dir_all(&config.state_dir)?;
        // `memory_db` (`db_filename = ":memory:"`) → base volatile,
        // comme `Database::memory()` des tests.
        let db = if config.db_filename == ":memory:" {
            Database::memory()?
        } else {
            Database::open(&config.db_path())?
        };
        let engine = BtEngine::start(config.engine.clone()).await?;
        let db = Arc::new(db);
        let asyncio = crate::asyncio::AsyncioMonitor::new(config.ipv8.walker_interval);
        let ipv8 = start_ipv8(&config, db.clone(), notifier.clone(), asyncio.tasks.clone()).await?;
        let services_config = config.clone();
        let session = Self {
            inner: Arc::new(Inner {
                config,
                overrides: std::sync::RwLock::new(ServiceOverrides::default()),
                engine,
                db,
                notifier,
                services: std::sync::Mutex::new(Services::default()),
                ipv8,
                last_tracker_sync: std::sync::Mutex::new(None),
                asyncio,
                stopped: std::sync::atomic::AtomicBool::new(false),
                restore_done: tokio::sync::watch::channel(false).0,
            }),
        };
        session.start_services(&services_config).await;
        session.spawn_restore();
        session.spawn_progress_loop();
        session.inner.notifier.notify(Notification::SessionStarted);
        Ok(session)
    }

    /// Session de test entierement en memoire (base volatile, moteur
    /// offline).
    pub async fn start_offline(config: CoreConfig, notifier: Notifier) -> Result<Self> {
        let db = Database::memory()?;
        let engine = BtEngine::start(config.engine.clone()).await?;
        let db = Arc::new(db);
        let asyncio = crate::asyncio::AsyncioMonitor::new(config.ipv8.walker_interval);
        let ipv8 = start_ipv8(&config, db.clone(), notifier.clone(), asyncio.tasks.clone()).await?;
        let services_config = config.clone();
        let session = Self {
            inner: Arc::new(Inner {
                config,
                overrides: std::sync::RwLock::new(ServiceOverrides::default()),
                engine,
                db,
                notifier,
                services: std::sync::Mutex::new(Services::default()),
                ipv8,
                last_tracker_sync: std::sync::Mutex::new(None),
                asyncio,
                stopped: std::sync::atomic::AtomicBool::new(false),
                // Base memoire : rien a restaurer — restauration
                // marquee terminee d'emblee.
                restore_done: tokio::sync::watch::channel(true).0,
            }),
        };
        session.start_services(&services_config).await;
        session.spawn_progress_loop();
        Ok(session)
    }

    /// Lance la restauration des telechargements persistes en tache
    /// de fond (`load_checkpoint` Python : un `TaskManager` anonyme,
    /// les downloads apparaissent au fil de leur re-add — la
    /// verification initiale des pieces rqbit peut durer plusieurs
    /// secondes par gros torrent et bloquait le bind de l'API).
    /// `restore_done` bascule a `true` en fin de tache.
    fn spawn_restore(&self) {
        let session = self.clone();
        self.inner
            .asyncio
            .tasks
            .register(Some("CoreSession"), "load_checkpoint", None);
        tokio::spawn(async move {
            session.restore_downloads().await;
            let _ = session.inner.restore_done.send(true);
        });
    }

    /// Attend la fin de la restauration des telechargements persistes
    /// (tests, diagnostics). Immediate si la restauration est deja
    /// terminee ou si la session n'en avait pas (base memoire).
    pub async fn wait_restored(&self) {
        let mut rx = self.inner.restore_done.subscribe();
        if *rx.borrow() {
            return;
        }
        // Err(_) = emetteur perdu : la session est detruite, on
        // considere la restauration close pour ne pas bloquer.
        let _ = rx.changed().await;
    }

    /// Reinjecte dans les moteurs les telechargements persistes, avec
    /// tous leurs reglages (equivalent du `load_checkpoint` Python :
    /// selection de fichiers, trackers additionnels, limites,
    /// dossier de sortie et etat pause sont reappliques a l'ajout).
    async fn restore_downloads(&self) {
        let rows = match self.inner.db.with(tribler_db::downloads::list) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "lecture des downloads persistes impossible");
                return;
            }
        };
        for row in rows {
            // `stop()` pendant la restauration : on abandonne — les
            // downloads non reinjectes seront relus au prochain run.
            if self.inner.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("restauration interrompue par l'arret de la session");
                return;
            }
            let engine = match self.engine_for(row.anon_hops as u32).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        infohash = %hex::encode(&row.infohash),
                        error = %e,
                        "moteur anonyme indisponible a la restauration"
                    );
                    continue;
                }
            };
            match self.readd_row(&engine, &row).await {
                Ok(dl) => {
                    tracing::info!(
                        infohash = %dl.info_hash_hex(),
                        "telechargement restaure"
                    );
                    if let Some(name) = dl.name() {
                        self.index_channel_node(&dl.info_hash(), &name, dl.stats().total_bytes);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        infohash = %hex::encode(&row.infohash),
                        error = %e,
                        "restauration d'un telechargement echouee"
                    );
                    // `on_tribler_exception` Python : les erreurs de
                    // restauration sont remontees au GUI.
                    self.inner.notifier.notify(Notification::TriblerException {
                        error: format!("restore {}: {e}", hex::encode(&row.infohash)),
                    });
                }
            }
        }
    }

    /// Options rqbit reconstruites depuis la ligne persistee — les
    /// reglages par telechargement (selection de fichiers, trackers
    /// ajoutes a chaud, limites, dossier de sortie, pause) sont
    /// reappliques a chaque (re)creation, comme le `DownloadConfig`
    /// checkpointe Python.
    fn row_add_options(row: &DownloadRow) -> AddDownloadOptions {
        AddDownloadOptions {
            paused: row.paused,
            output_folder: (!row.output_dir.is_empty()).then(|| PathBuf::from(&row.output_dir)),
            only_files: row
                .selected_files
                .as_ref()
                .map(|l| l.iter().map(|&i| i as usize).collect()),
            trackers: crate::trackers::effective_trackers(row),
            upload_limit_bps: u64::try_from(row.upload_limit).ok().filter(|&v| v > 0),
            download_limit_bps: u64::try_from(row.download_limit).ok().filter(|&v| v > 0),
        }
    }

    /// Recree le telechargement decrit par `row` sur `engine`
    /// (`.torrent` persiste en priorite, `source_uri` sinon). Les
    /// `removed_trackers` sont appliques a la source elle-meme —
    /// librqbit refusionne les trackers de la source avec
    /// `opts.trackers` : seule une source purgee les honore.
    async fn readd_row(&self, engine: &BtEngine, row: &DownloadRow) -> Result<Download> {
        let opts = Self::row_add_options(row);
        let (torrent_data, source_uri) = crate::trackers::effective_source(row);
        Ok(if let Some(bytes) = torrent_data {
            engine.add_torrent_bytes_opts(bytes, &opts).await?
        } else {
            engine.add_uri_opts(&source_uri, &opts).await?
        })
    }

    /// Ligne de persistance d'un telechargement (`None` si inconnue).
    fn row_of(&self, ih: &[u8]) -> Result<Option<DownloadRow>> {
        Ok(self.inner.db.with(|c| tribler_db::downloads::get(c, ih))?)
    }

    /// Lecture-modification-ecriture des reglages persistes d'un
    /// telechargement (`true` si la ligne existait). Les colonnes
    /// runtime de la ligne relue sont conservees — `f` ne touche que
    /// les reglages.
    pub fn update_download_row(&self, ih: &[u8], f: impl FnOnce(&mut DownloadRow)) -> Result<bool> {
        Ok(self.inner.db.with(|c| {
            let Some(mut row) = tribler_db::downloads::get(c, ih)? else {
                return Ok(false);
            };
            f(&mut row);
            tribler_db::downloads::upsert(c, &row)?;
            Ok(true)
        })?)
    }

    /// Boucle periodique : publie `DownloadProgress` pour chaque
    /// telechargement actif et detecte les fins de telechargement.
    fn spawn_progress_loop(&self) {
        let session = self.clone();
        let interval = std::time::Duration::from_millis(self.inner.config.progress_interval_ms);
        self.inner.asyncio.tasks.register(
            Some("CoreSession"),
            "progress",
            Some(interval.as_secs_f64()),
        );
        tokio::spawn(async move {
            let mut finished: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut queue_paused: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut backed_up: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                for stats in session.downloads() {
                    let newly_done = stats.finished && !finished.contains(&stats.info_hash);
                    if newly_done {
                        finished.insert(stats.info_hash.clone());
                        session
                            .inner
                            .notifier
                            .notify(Notification::DownloadFinished {
                                infohash: stats.info_hash.clone(),
                                name: stats.name.clone(),
                            });
                        let ih = tribler_crypto::hash::from_hex(&stats.info_hash);
                        if let Some(ih) = ih {
                            let _ = session
                                .inner
                                .db
                                .with(|c| tribler_db::downloads::set_finished(c, &ih, true));
                            // `add_download_to_channel` Python : les
                            // canaux ne sont pas portes — l'attribut
                            // persiste en base et le manque est trace.
                            let channel = session
                                .inner
                                .db
                                .with(|c| tribler_db::downloads::get(c, &ih))
                                .ok()
                                .flatten()
                                .map(|r| r.add_download_to_channel)
                                .unwrap_or(false);
                            if channel {
                                tracing::debug!(
                                    infohash = %stats.info_hash,
                                    "add_download_to_channel : les canaux ne sont pas implementes"
                                );
                            }
                        }
                        // `libtorrent/check_after_complete` Python :
                        // reverification des pieces a la fin
                        // (`session.recheck` = remove + re-add, le
                        // hash-check rqbit sert de recheck).
                        if session.inner.config.check_after_complete {
                            let session = session.clone();
                            let ih = stats.info_hash.clone();
                            tokio::spawn(async move {
                                if let Err(e) = session.recheck(&ih).await {
                                    tracing::warn!(
                                        error = %e,
                                        infohash = %ih,
                                        "check_after_complete en erreur"
                                    );
                                }
                            });
                        }
                    }
                    session.enforce_seeding_policy(&stats);
                    // `download_defaults/torrent_folder` Python
                    // (`PostHandleOp.WRITE_BACKUP_TORRENT`) : sauvegarde
                    // du .torrent des que le metainfo est connu.
                    if !session
                        .inner
                        .config
                        .download_defaults
                        .torrent_folder
                        .is_empty()
                        && !backed_up.contains(&stats.info_hash)
                        && session.backup_torrent_file(&stats.info_hash)
                    {
                        backed_up.insert(stats.info_hash.clone());
                    }
                    session
                        .inner
                        .notifier
                        .notify(Notification::DownloadProgress(stats));
                }
                session.enforce_queue_limits(&mut queue_paused).await;
            }
        });
    }

    /// `write_backup_torrent_file` Python : ecrit
    /// `<name> [<infohash hex>].torrent` dans
    /// `download_defaults/torrent_folder` quand le metainfo est connu.
    /// Retourne `false` quand le metainfo n'est pas encore la (magnet
    /// non resolu — a reessayer au prochain tick) ou `true` sinon.
    fn backup_torrent_file(&self, ih_hex: &str) -> bool {
        let folder = &self.inner.config.download_defaults.torrent_folder;
        let Some(dl) = self.find_download_hex(ih_hex) else {
            return true; // disparu entre-temps — ne pas reessayer
        };
        let Some(bytes) = dl.torrent_bytes() else {
            return false; // metainfo pas encore arrive (magnet)
        };
        let name = dl
            .name()
            .unwrap_or_else(|| ih_hex.to_string())
            .replace(['/', '\\'], "_");
        let path = Path::new(folder).join(format!("{name} [{ih_hex}].torrent"));
        if let Err(e) = std::fs::create_dir_all(folder).and_then(|_| std::fs::write(&path, &bytes))
        {
            tracing::warn!(error = %e, path = %path.display(), "sauvegarde .torrent impossible");
        }
        true
    }

    /// Acces au bus d'evenements.
    pub fn notifier(&self) -> &Notifier {
        &self.inner.notifier
    }

    /// Acces direct au moteur (services internes : watch folder,
    /// torrent checker…).
    pub fn engine(&self) -> &BtEngine {
        &self.inner.engine
    }

    /// Moteur cible pour un telechargement : principal si
    /// `anon_hops == 0`, sinon la lane anonyme a `hops` sauts
    /// (SOCKS5 de la `TunnelCommunity` + moteur uTP-only dedie —
    /// equivalent des sessions libtorrent `hops=` de Tribler).
    ///
    /// Erreur si `anon_hops > 0` et que l'anonymat n'est pas active
    /// (`ipv8.enable_anonymity`), comme `anon_hops` refuse sans tunnel
    /// cote Python.
    pub async fn engine_for(&self, anon_hops: u32) -> Result<BtEngine> {
        if anon_hops == 0 {
            return Ok(self.inner.engine.clone());
        }
        let stack = self.inner.ipv8.clone().ok_or(CoreError::InvalidState(
            "anon_hops > 0 mais la stack ipv8 est inactive",
        ))?;
        stack.anon_engine(anon_hops as usize).await
    }

    /// Tous les moteurs (principal + lanes anonymes actives).
    fn all_engines(&self) -> Vec<BtEngine> {
        let mut engines = vec![self.inner.engine.clone()];
        if let Some(stack) = &self.inner.ipv8 {
            engines.extend(stack.anon_engines());
        }
        engines
    }

    /// Telechargement par info-hash ou id interne (tous moteurs —
    /// `/api/downloads/{ih}/*`).
    pub fn find_download(&self, id_or_hash: &str) -> Option<Download> {
        self.all_engines().iter().find_map(|e| e.get(id_or_hash))
    }

    /// Lookup strict par info-hash hex (`unhexlify` Python) — toutes
    /// les routes `/api/downloads/{ih}` doivent l'utiliser : un hash
    /// tout-chiffres comme `"00..0"` ne doit jamais etre interprete
    /// comme l'id interne 0 (`parse_id_or_hash` le ferait).
    pub fn find_download_hex(&self, hex: &str) -> Option<Download> {
        let ih = tribler_crypto::hash::from_hex(hex)?;
        self.all_engines().iter().find_map(|e| e.get_by_hash(&ih))
    }

    /// Politique d'arret de seed — `download_defaults/seeding_mode`
    /// global plus `seeding_ratio` individuel (`DownloadConfig`).
    /// Appelee a chaque tick pour les telechargements termines :
    /// - `never` stoppe le seed a la fin du telechargement ;
    /// - `ratio` quand upload/download >= la cible (ratio individuel
    ///   en priorite, sinon le defaut) ;
    /// - `time` apres `seeding_time` secondes de seed ;
    /// - `forever` ne stoppe que sur ratio individuel explicite.
    fn enforce_seeding_policy(&self, stats: &DownloadStats) {
        if !stats.finished {
            return;
        }
        let Some(ih) = tribler_crypto::hash::from_hex(&stats.info_hash) else {
            return;
        };
        let row = self
            .inner
            .db
            .with(|c| tribler_db::downloads::get(c, &ih))
            .unwrap_or(None);
        let Some(row) = row else { return };
        if row.paused || row.user_stopped {
            return;
        }
        let dd = &self.inner.config.download_defaults;
        let ratio_target = row
            .seeding_ratio
            .filter(|r| *r > 0.0)
            .or_else(|| (dd.seeding_mode == "ratio").then_some(dd.seeding_ratio));
        let stop = if let Some(ratio) = ratio_target {
            stats.total_bytes > 0 && stats.uploaded_bytes as f64 >= ratio * stats.total_bytes as f64
        } else {
            match dd.seeding_mode.as_str() {
                "never" => true,
                "time" => {
                    row.time_finished > 0
                        && now_unix().saturating_sub(row.time_finished) >= dd.seeding_time as i64
                }
                _ => false,
            }
        };
        if !stop {
            return;
        }
        if let Some(engine) = self.owner_engine(&stats.info_hash) {
            let ih_hex = stats.info_hash.clone();
            tokio::spawn(async move {
                if let Err(e) = engine.pause(&ih_hex).await {
                    tracing::warn!(error = %e, "arret de seed automatique impossible");
                }
            });
            let _ = self.update_download_row(&ih, |r| r.paused = true);
        }
    }

    /// Gestionnaire de file libtorrent (`active_downloads` /
    /// `active_seeds` / `active_limit`) : librqbit n'a pas de file —
    /// au-dela des bornes, les telechargements `auto_managed` en fin
    /// de file (`queue_position` decroissante) sont mis en pause par
    /// le moteur ; quand des slots se liberent ils sont repris par
    /// `queue_position` croissante. Les pauses de file ne touchent pas
    /// `paused`/`user_stopped` persistes (distinction `queued` Python)
    /// ; si l'utilisateur pause ou reprend un torrent mis en file, la
    /// ligne reprend la main et l'entree est purgee de `queue_paused`.
    async fn enforce_queue_limits(&self, queue_paused: &mut std::collections::HashSet<String>) {
        // Config effective : les `active_*` modifies par
        // `POST /api/settings` s'appliquent des le tick suivant
        // (`set_session_limits` Python).
        let effective = self.effective_config();
        let q = &effective.queue;
        if q.disabled() {
            queue_paused.clear();
            return;
        }
        let rows = self
            .inner
            .db
            .with(tribler_db::downloads::list)
            .unwrap_or_default();
        let row_of = |hash: &str| {
            tribler_crypto::hash::from_hex(hash)
                .and_then(|ih| rows.iter().find(|r| r.infohash == ih))
        };
        // La ligne persistante reprend la main : purge des pauses de
        // file dont le telechargement a ete pause par l'utilisateur.
        queue_paused.retain(|h| row_of(h).is_some_and(|r| !r.paused && !r.user_stopped));
        for engine in self.all_engines() {
            let stats = engine.list();
            let known: std::collections::HashSet<&str> =
                stats.iter().map(|s| s.info_hash.as_str()).collect();
            queue_paused.retain(|h| known.contains(h.as_str()));
            // Actifs de la file : seuls les `auto_managed` comptent
            // dans les bornes (regle libtorrent — les autres sont
            // totalement exempts de la file).
            let mut n_dl = 0i64;
            let mut n_seed = 0i64;
            let mut running: Vec<(i64, String, bool)> = Vec::new();
            for s in &stats {
                let Some(row) = row_of(&s.info_hash) else {
                    continue;
                };
                if !row.auto_managed
                    || row.paused
                    || row.user_stopped
                    || queue_paused.contains(&s.info_hash)
                {
                    continue;
                }
                match s.state {
                    DownloadState::Downloading
                    | DownloadState::Checking
                    | DownloadState::Initializing
                    | DownloadState::Seeding => {
                        if s.finished {
                            n_seed += 1;
                        } else {
                            n_dl += 1;
                        }
                        running.push((row.queue_position, s.info_hash.clone(), s.finished));
                    }
                    _ => {}
                }
            }
            // Pause de l'excedent en partant de la fin de la file.
            let mut excess_dl = if q.active_downloads >= 0 {
                (n_dl - q.active_downloads).max(0)
            } else {
                0
            };
            let mut excess_seed = if q.active_seeds >= 0 {
                (n_seed - q.active_seeds).max(0)
            } else {
                0
            };
            let mut excess_total = if q.active_limit >= 0 {
                (n_dl + n_seed - q.active_limit).max(0)
            } else {
                0
            };
            running.sort_by_key(|e| std::cmp::Reverse(e.0));
            let mut paused_now = 0usize;
            for (_, hash, is_seed) in &running {
                let over = if *is_seed {
                    excess_seed > 0 || excess_total > 0
                } else {
                    excess_dl > 0 || excess_total > 0
                };
                if !over {
                    break; // tri desc : les plus prioritaires sont gardes
                }
                if let Err(e) = engine.pause(hash).await {
                    tracing::warn!(error = %e, infohash = %hash, "pause de file impossible");
                } else {
                    queue_paused.insert(hash.clone());
                    paused_now += 1;
                    if *is_seed {
                        excess_seed -= 1;
                    } else {
                        excess_dl -= 1;
                    }
                    excess_total -= 1;
                }
            }
            if paused_now > 0 {
                continue; // les reprises seront evaluees au prochain tick
            }
            // Reprise des pauses de file quand des slots se liberent.
            let mut free_dl = if q.active_downloads >= 0 {
                q.active_downloads - n_dl
            } else {
                i64::MAX
            };
            let mut free_seed = if q.active_seeds >= 0 {
                q.active_seeds - n_seed
            } else {
                i64::MAX
            };
            let mut free_total = if q.active_limit >= 0 {
                q.active_limit - n_dl - n_seed
            } else {
                i64::MAX
            };
            let mut queued: Vec<(i64, String, bool)> = queue_paused
                .iter()
                .filter(|h| known.contains(h.as_str()))
                .map(|h| {
                    let done = stats
                        .iter()
                        .find(|s| s.info_hash == *h)
                        .map(|s| s.finished)
                        .unwrap_or(false);
                    (
                        row_of(h).map(|r| r.queue_position).unwrap_or(i64::MAX),
                        h.clone(),
                        done,
                    )
                })
                .collect();
            queued.sort_by_key(|e| e.0);
            for (_, hash, is_seed) in queued {
                let slot = if is_seed {
                    free_seed > 0 && free_total > 0
                } else {
                    free_dl > 0 && free_total > 0
                };
                if !slot {
                    continue;
                }
                match engine.resume(&hash).await {
                    Ok(()) => {
                        queue_paused.remove(&hash);
                        free_seed -= i64::from(is_seed);
                        free_dl -= i64::from(!is_seed);
                        free_total -= 1;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, infohash = %hash, "reprise de file impossible")
                    }
                }
            }
        }
    }

    /// Ajoute un telechargement (magnet ou URI `http(s)`), eventuellement
    /// anonyme (`anon_hops` sauts de tunnel — necessite
    /// `ipv8.enable_anonymity`), et le persiste.
    pub async fn add_download(&self, uri: &str, paused: bool) -> Result<Download> {
        let safe = self.inner.config.download_defaults.safeseeding_enabled;
        self.add_download_anon(uri, paused, 0, safe, None).await
    }

    /// `add_download` avec choix du nombre de sauts anonymes et du
    /// flag `safe_seeding` (persiste dans la ligne `downloads`).
    /// `destination` = parametre `destination` de `PUT /api/downloads`
    /// (`set_dest_dir` Python) ; `None` = `saveas` effectif
    /// (override `POST /api/settings` puis dossier de session).
    pub async fn add_download_anon(
        &self,
        uri: &str,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
    ) -> Result<Download> {
        if anon_hops == 0 {
            self.check_uri_policy(uri).await?;
        }
        self.check_low_space();
        let engine = self.engine_for(anon_hops).await?;
        // Trackers par defaut (`trackers_file`) : ajoutes a chaque
        // nouveau telechargement, comme le post-handle
        // `ADD_DEFAULT_TRACKERS` Python — et persistes dans
        // `extra_trackers` pour survivre aux re-adds.
        let trackers = self.default_trackers().await;
        let dl = engine
            .add_uri_opts(
                uri,
                &AddDownloadOptions {
                    paused,
                    output_folder: self.effective_output_dir(destination),
                    trackers: trackers.clone(),
                    ..Default::default()
                },
            )
            .await?;
        self.persist(
            &dl,
            uri,
            PersistParams {
                paused,
                anon_hops,
                safe_seeding,
                extra_trackers: trackers,
            },
        )?;
        Ok(dl)
    }

    /// Trackers du `download_defaults/trackers_file` (relatif a
    /// `state_dir`), ajoutes a chaque nouveau telechargement comme
    /// le post-handle `ADD_DEFAULT_TRACKERS` Python. Synchronise
    /// d'abord le fichier depuis `trackers_file_sync_url` si configure
    /// (`sync_default_trackers_file` : un fetch par heure maximum).
    async fn default_trackers(&self) -> Vec<String> {
        let dd = &self.inner.config.download_defaults;
        let Some(path) =
            crate::trackers::trackers_file_path(&self.inner.config.state_dir, &dd.trackers_file)
        else {
            return Vec::new();
        };
        if !dd.trackers_file_sync_url.is_empty() {
            self.sync_trackers_file(&dd.trackers_file_sync_url, &path)
                .await;
        }
        std::fs::read_to_string(&path)
            .map(|s| crate::trackers::parse_trackers_file(&s))
            .unwrap_or_default()
    }

    /// `sync_default_trackers_file` Python : reecrit `trackers_file`
    /// avec le contenu de `trackers_file_sync_url`, au plus une fois
    /// par [`crate::trackers::TRACKER_SYNC_TTL_SECS`]. Les erreurs
    /// sont loggees puis ignorees — le fichier precedent reste en
    /// vigueur, comme Python.
    async fn sync_trackers_file(&self, sync_url: &str, path: &std::path::Path) {
        let recent = self
            .inner
            .last_tracker_sync
            .lock()
            .ok()
            .and_then(|g| *g)
            .is_some_and(|t| t.elapsed().as_secs() < crate::trackers::TRACKER_SYNC_TTL_SECS);
        if recent {
            return;
        }
        // Anti-SSRF : la cible est validee par la meme politique IP
        // que les URI de telechargement ajoutees via l'API.
        if let Err(e) = self.check_uri_policy(sync_url).await {
            tracing::warn!(error = %e, "synchronisation trackers_file refusee");
            return;
        }
        match reqwest::get(sync_url).await {
            Ok(resp) => match resp.bytes().await {
                Ok(body) => {
                    if let Err(e) = std::fs::write(path, &body) {
                        tracing::warn!(error = %e, "ecriture de trackers_file impossible");
                    }
                    if let Ok(mut g) = self.inner.last_tracker_sync.lock() {
                        *g = Some(std::time::Instant::now());
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "synchronisation trackers_file : corps illisible")
                }
            },
            Err(e) => tracing::warn!(error = %e, "synchronisation trackers_file echouee"),
        }
    }

    /// `add_tracker` Python (`PUT /downloads/{ih}/trackers`) : ajoute
    /// l'URL a l'overlay du telechargement actif et la persiste dans
    /// `extra_trackers` (rejouee au re-add — rqbit ne reannonce pas un
    /// tracker ajoute a chaud, divergence documentee).
    pub async fn add_tracker(&self, id_or_hash: &str, url: &str) -> Result<()> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        dl.add_tracker(url);
        let ih = dl.info_hash();
        self.update_download_row(&ih, |r| {
            r.removed_trackers.retain(|u| u != url);
            if !r.extra_trackers.iter().any(|u| u == url) {
                r.extra_trackers.push(url.to_string());
            }
        })?;
        Ok(())
    }

    /// `remove_tracker` Python (`DELETE /downloads/{ih}/trackers`) :
    /// retire l'URL des trackers effectifs — d'`extra_trackers` si
    /// c'est un ajout a chaud, sinon enregistree dans
    /// `removed_trackers` (filtree de la source au re-add). Le torrent
    /// actif continue d'annoncer jusqu'a sa recreation : librqbit
    /// n'expose pas `replace_trackers` (divergence documentee).
    /// Une URL inconnue est un no-op, comme `replace_trackers` Python.
    pub async fn remove_tracker(&self, id_or_hash: &str, url: &str) -> Result<()> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        dl.remove_extra_tracker(url);
        let ih = dl.info_hash();
        let known = self
            .row_of(&ih)?
            .map(|r| crate::trackers::source_trackers(&r))
            .unwrap_or_default();
        self.update_download_row(&ih, |r| {
            if let Some(i) = r.extra_trackers.iter().position(|u| u == url) {
                r.extra_trackers.remove(i);
            } else if known.iter().any(|u| u == url) && !r.removed_trackers.iter().any(|u| u == url)
            {
                r.removed_trackers.push(url.to_string());
            }
        })?;
        Ok(())
    }

    /// `add_default_trackers` Python (`PUT /downloads/{ih}/default_trackers`
    /// et post-handle auto a l'ajout) : ajoute les trackers du fichier
    /// configure a l'overlay et a la persistance.
    pub async fn add_default_trackers(&self, id_or_hash: &str) -> Result<()> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let defaults = self.default_trackers().await;
        if defaults.is_empty() {
            return Ok(());
        }
        for url in &defaults {
            dl.add_tracker(url);
        }
        let ih = dl.info_hash();
        self.update_download_row(&ih, |r| {
            for url in &defaults {
                r.removed_trackers.retain(|u| u != url);
                if !r.extra_trackers.iter().any(|u| u == url) {
                    r.extra_trackers.push(url.clone());
                }
            }
        })?;
        Ok(())
    }

    /// `tracker_force_announce` Python (`PUT .../tracker_force_announce`)
    /// : force une re-annonce. Notre implementation reannonce tous les
    /// trackers (pause+unpause rqbit — superset du
    /// `force_reannounce(0, i)` cible ; ecart documente).
    pub async fn force_announce(&self, id_or_hash: &str) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        Ok(engine.force_announce(id_or_hash).await?)
    }

    /// Anti-SSRF : une URI `http(s)` (fournie par un tiers via
    /// l'API) ne doit jamais faire ressortir une requete vers une
    /// adresse refusee par `config.ip_policy`. Resolution DNS puis
    /// refus ferme : TOUTE adresse resolue doit etre autorisee.
    async fn check_uri_policy(&self, uri: &str) -> Result<()> {
        if !uri.starts_with("http://") && !uri.starts_with("https://") {
            return Ok(());
        }
        let parsed =
            url::Url::parse(uri).map_err(|_| CoreError::InvalidState("uri http(s) invalide"))?;
        let host = parsed
            .host_str()
            .ok_or(CoreError::InvalidState("uri http(s) sans hote"))?;
        let port = parsed
            .port_or_known_default()
            .ok_or(CoreError::InvalidState("uri http(s) sans port"))?;
        let mut count = 0usize;
        for addr in tokio::net::lookup_host((host, port)).await? {
            self.inner.config.ip_policy.check(&addr)?;
            count += 1;
        }
        if count == 0 {
            return Err(CoreError::InvalidState("uri http(s) sans adresse"));
        }
        Ok(())
    }

    /// Ajoute un telechargement depuis les octets d'un `.torrent`.
    pub async fn add_torrent_bytes(&self, bytes: Vec<u8>, paused: bool) -> Result<Download> {
        let safe = self.inner.config.download_defaults.safeseeding_enabled;
        self.add_torrent_bytes_anon(bytes, paused, 0, safe, None)
            .await
    }

    /// `add_torrent_bytes` avec choix du nombre de sauts anonymes et
    /// du flag `safe_seeding` persiste. `destination` : voir
    /// [`Session::add_download_anon`].
    pub async fn add_torrent_bytes_anon(
        &self,
        bytes: Vec<u8>,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
    ) -> Result<Download> {
        // Parsing borne en amont pour extraire l'info-hash a persister.
        let meta = tribler_format::torrent::TorrentMeta::parse(&bytes)?;
        self.check_low_space();
        let engine = self.engine_for(anon_hops).await?;
        // Pas de trackers par defaut sur un torrent prive (condition
        // `not torrent_info.priv()` du `_post_handle_events` Python).
        let trackers = if meta.private {
            Vec::new()
        } else {
            self.default_trackers().await
        };
        let dl = engine
            .add_torrent_bytes_opts(
                bytes.clone(),
                &AddDownloadOptions {
                    paused,
                    output_folder: self.effective_output_dir(destination),
                    trackers: trackers.clone(),
                    ..Default::default()
                },
            )
            .await?;
        self.persist_torrent(
            &dl,
            bytes,
            &meta,
            PersistParams {
                paused,
                anon_hops,
                safe_seeding,
                extra_trackers: trackers,
            },
        )?;
        Ok(dl)
    }

    /// Dossier de sortie d'un nouveau telechargement : `destination`
    /// explicite (`PUT /api/downloads`), sinon le `saveas` effectif
    /// (override `POST /api/settings`), sinon `None` = dossier de
    /// l'engine (par lane). Python : `DownloadConfig.destination`
    /// defaut = `libtorrent/download_defaults/saveas`.
    fn effective_output_dir(
        &self,
        destination: Option<std::path::PathBuf>,
    ) -> Option<std::path::PathBuf> {
        destination.or_else(|| {
            self.inner
                .overrides
                .read()
                .ok()
                .and_then(|ov| ov.download_dir.clone())
        })
    }

    /// Reglages initiaux d'une ligne `downloads` : defauts de
    /// `download_defaults` Python (`auto_managed`, `completed_dir`)
    /// et position en fin de file (`next_queue_position`).
    fn settings_defaults(
        &self,
        c: &rusqlite::Connection,
        safe_seeding: bool,
    ) -> tribler_db::Result<DownloadRow> {
        let dd = &self.inner.config.download_defaults;
        Ok(DownloadRow {
            safe_seeding,
            auto_managed: dd.auto_managed,
            queue_position: tribler_db::downloads::next_queue_position(c)?,
            completed_dir: (!dd.completed_dir.is_empty()).then(|| dd.completed_dir.clone()),
            channel_download: dd.channel_download,
            add_download_to_channel: dd.add_download_to_channel,
            ..Default::default()
        })
    }

    fn persist(&self, dl: &Download, uri: &str, p: PersistParams) -> Result<()> {
        self.inner.db.with(|c| {
            tribler_db::downloads::upsert(
                c,
                &DownloadRow {
                    infohash: dl.info_hash().to_vec(),
                    name: dl.name(),
                    source_uri: uri.to_string(),
                    output_dir: dl.output_folder().display().to_string(),
                    added_on: now_unix(),
                    paused: p.paused,
                    anon_hops: i64::from(p.anon_hops),
                    extra_trackers: p.extra_trackers,
                    ..self.settings_defaults(c, p.safe_seeding)?
                },
            )
        })?;
        if let Some(name) = dl.name() {
            self.index_channel_node(&dl.info_hash(), &name, dl.stats().total_bytes);
        }
        Ok(())
    }

    fn persist_torrent(
        &self,
        dl: &Download,
        bytes: Vec<u8>,
        meta: &tribler_format::torrent::TorrentMeta,
        p: PersistParams,
    ) -> Result<()> {
        self.inner.db.with(|c| {
            tribler_db::downloads::upsert(
                c,
                &DownloadRow {
                    infohash: meta.info_hash.to_vec(),
                    name: Some(meta.name.clone()),
                    source_uri: format!(
                        "magnet:?xt=urn:btih:{}",
                        tribler_crypto::hash::to_hex(&meta.info_hash)
                    ),
                    torrent_data: Some(bytes),
                    output_dir: dl.output_folder().display().to_string(),
                    added_on: now_unix(),
                    paused: p.paused,
                    anon_hops: i64::from(p.anon_hops),
                    extra_trackers: p.extra_trackers,
                    ..self.settings_defaults(c, p.safe_seeding)?
                },
            )
        })?;
        self.index_channel_node(&meta.info_hash, &meta.name, meta.total_size);
        Ok(())
    }

    /// Indexe les metadonnees du torrent dans `channel_node` pour alimenter
    /// les recherches locales (`/api/metadata/search/local`) et populaires.
    fn index_channel_node(&self, infohash: &[u8], name: &str, size: u64) {
        let _ = self.inner.db.with(|c| {
            if let Ok(Some(_)) = tribler_db::channel::get_by_infohash(c, infohash) {
                return Ok(());
            }
            let row = tribler_db::models::ChannelNodeRow {
                infohash: infohash.to_vec(),
                size: size as i64,
                torrent_date: now_unix(),
                title: name.to_string(),
                metadata_type: 300, // Torrent regular
                status: 1,          // COMMITTED
                origin_id: 0,
                public_key: vec![0u8; 64],
                id_: now_unix()
                    .wrapping_mul(1000)
                    .wrapping_add(rand::random::<i16>() as i64)
                    .abs(),
                timestamp: now_unix() * 1000,
                added_on: now_unix(),
                ..Default::default()
            };
            let _ = tribler_db::channel::insert(c, &row);
            Ok(())
        });
    }

    /// `update_hops` Python (`DownloadManager.update_hops`) : retire le
    /// telechargement de son moteur actuel puis le recree sur le moteur
    /// a `new_hops` sauts — les donnees sur disque ET les reglages
    /// persistes sont conserves (comme le `checkpoint` Python survive
    /// a la recreation).
    pub async fn update_hops(&self, id_or_hash: &str, new_hops: u32) -> Result<()> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let ih = dl.info_hash();
        let mut row = self
            .row_of(&ih)?
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        // Metainfo resolue depuis : la conserver permet les re-adds
        // ulterieurs hors-ligne (meme source que Python : le .torrent
        // checkpointe).
        if row.torrent_data.is_none() {
            row.torrent_data = dl.torrent_bytes().map(|b| b.to_vec());
        }
        let old_hops = row.anon_hops.max(0) as u32;
        let old_engine = self.engine_for(old_hops).await?;
        let new_engine = self.engine_for(new_hops).await?;
        // Fastresume inter-moteurs : le `.bitv` de rqbit est supprime
        // du dossier de l'ancienne session au `delete` — on le met de
        // cote puis on le depose dans le dossier de la nouvelle lane
        // AVANT le re-add pour court-circuiter le re-hash complet.
        let ih_hex = tribler_crypto::hash::to_hex(&ih);
        let bitv = match old_engine.config().persistence_dir.as_ref() {
            Some(dir) => tokio::fs::read(dir.join(format!("{ih_hex}.bitv")))
                .await
                .ok(),
            None => None,
        };
        self.remove_engine_only(id_or_hash, false).await?;
        if let (Some(dir), Some(bits)) = (new_engine.config().persistence_dir.clone(), bitv) {
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::warn!(error = %e, "update_hops: dossier fastresume increatable");
            } else if let Err(e) = tokio::fs::write(dir.join(format!("{ih_hex}.bitv")), &bits).await
            {
                tracing::warn!(error = %e, "update_hops: transfert fastresume impossible");
            }
        }
        row.anon_hops = i64::from(new_hops);
        match self.readd_row(&new_engine, &row).await {
            Ok(_) => {
                self.inner
                    .db
                    .with(|c| tribler_db::downloads::upsert(c, &row))?;
                Ok(())
            }
            Err(e) => {
                // Rollback best-effort sur l'ancien moteur : Python
                // laisse le download perdu, on prefere restaurer
                // l'etat initial (ligne DB intacte).
                row.anon_hops = i64::from(old_hops);
                if let Err(rb) = self.readd_row(&old_engine, &row).await {
                    tracing::warn!(
                        infohash = %tribler_crypto::hash::to_hex(&ih),
                        error = %rb,
                        "update_hops: rollback impossible — download perdu"
                    );
                }
                Err(e)
            }
        }
    }

    /// Liste les telechargements (moteur principal + lanes anonymes).
    pub fn downloads(&self) -> Vec<DownloadStats> {
        self.all_engines().iter().flat_map(|e| e.list()).collect()
    }

    /// Stats par pair d'un telechargement (tous moteurs — principal
    /// et lanes anonymes ; `get_peers` de `GET /api/downloads`).
    pub fn peer_stats(&self, id_or_hash: &str) -> Option<Vec<tribler_bittorrent::DownloadPeer>> {
        self.all_engines()
            .iter()
            .find_map(|e| e.peer_stats(id_or_hash))
    }

    /// Bitfield base64 MSB-first des pieces detenues (tous moteurs —
    /// `get_pieces_base64` Python).
    pub fn have_pieces_base64(&self, id_or_hash: &str) -> Option<String> {
        self.all_engines()
            .iter()
            .find_map(|e| e.have_pieces_base64(id_or_hash))
    }

    /// `anon_hops` par info-hash hex, d'apres la persistance DB
    /// (utilise par `GET /api/downloads` pour `hops`/`anon_download`).
    pub fn anon_hops_map(&self) -> std::collections::HashMap<String, u32> {
        self.inner
            .db
            .with(tribler_db::downloads::list)
            .map(|rows| {
                rows.into_iter()
                    .map(|r| {
                        (
                            tribler_crypto::hash::to_hex(&r.infohash),
                            r.anon_hops.max(0) as u32,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Moteur detenant le telechargement `id_or_hash` (principal ou
    /// lane anonyme).
    fn owner_engine(&self, id_or_hash: &str) -> Option<BtEngine> {
        self.all_engines()
            .into_iter()
            .find(|e| e.get(id_or_hash).is_some())
    }

    /// Pause idempotente (`download.stop()` Python est un no-op sur
    /// un download deja arrete ; librqbit renvoie une erreur — on
    /// l'absorbe pour la parite).
    pub async fn pause(&self, id_or_hash: &str) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        if !engine
            .get(id_or_hash)
            .map(|d| d.is_paused())
            .unwrap_or(false)
        {
            engine.pause(id_or_hash).await?;
        }
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Reprend un telechargement (idempotent, comme `resume` Python).
    pub async fn resume(&self, id_or_hash: &str) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        if engine
            .get(id_or_hash)
            .map(|d| d.is_paused())
            .unwrap_or(false)
        {
            engine.resume(id_or_hash).await?;
        }
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Met en pause tous les telechargements de tous les moteurs
    /// (principal + lanes anonymes) — suspension mobile (Doze/iOS)
    /// et arret rapide. Retourne les erreurs unitaires (non fatales).
    pub async fn pause_all(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for e in self.all_engines() {
            errors.extend(e.pause_all().await);
        }
        errors
    }

    /// Reprend tous les telechargements en pause. Les lanes dont le
    /// kill switch est engage ne reprennent pas — leur erreur est
    /// collectee, les autres moteurs reprennent normalement.
    pub async fn resume_all(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for e in self.all_engines() {
            errors.extend(e.resume_all().await);
        }
        errors
    }

    /// Retire le telechargement de son moteur sans toucher a la ligne
    /// `downloads` — reserve aux recreations internes (`update_hops`,
    /// `recheck`, `move_storage`) qui reinjectent la ligne apres.
    async fn remove_engine_only(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        Ok(engine.remove(id_or_hash, delete_files).await?)
    }

    /// Supprime un telechargement (optionnellement ses fichiers) et
    /// sa ligne de persistance (`DELETE /api/downloads/{ih}`).
    pub async fn remove(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let infohash = self.find_download(id_or_hash).map(|d| d.info_hash_hex());
        self.remove_engine_only(id_or_hash, delete_files).await?;
        if let Some(h) = infohash {
            if let Some(ih) = tribler_crypto::hash::from_hex(&h) {
                let _ = self
                    .inner
                    .db
                    .with(|c| tribler_db::downloads::delete(c, &ih));
            }
        }
        Ok(())
    }

    fn notify_state(&self, id_or_hash: &str) {
        if let Some(engine) = self.owner_engine(id_or_hash) {
            if let Some(d) = engine.get(id_or_hash) {
                let s = d.stats();
                self.inner
                    .notifier
                    .notify(Notification::DownloadStateChanged {
                        infohash: s.info_hash,
                        state: s.state,
                    });
            }
        }
    }

    /// Ligne persistante + download actif d'un `id_or_hash`
    /// (couple requis par les operations de reglages du PATCH).
    fn download_and_row(&self, id_or_hash: &str) -> Result<(Download, DownloadRow)> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let row = self
            .row_of(&dl.info_hash())?
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        Ok((dl, row))
    }

    /// `force_recheck` Python : librqbit revérifie les pieces a
    /// chaque ajout (pas de fastresume persistant — `persistence`
    /// non configuree) → remove + re-add = revalidation complete.
    /// Les reglages de la ligne sont preserves.
    pub async fn recheck(&self, id_or_hash: &str) -> Result<()> {
        let (dl, mut row) = self.download_and_row(id_or_hash)?;
        let ih = dl.info_hash();
        if row.torrent_data.is_none() {
            row.torrent_data = dl.torrent_bytes().map(|b| b.to_vec());
        }
        let engine = self.engine_for(row.anon_hops.max(0) as u32).await?;
        self.remove_engine_only(id_or_hash, false).await?;
        self.readd_row(&engine, &row).await?;
        self.inner
            .db
            .with(|c| tribler_db::downloads::upsert(c, &row))?;
        self.notify_state(&tribler_crypto::hash::to_hex(&ih));
        Ok(())
    }

    /// `move_storage` Python : deplace les fichiers vers `dest_dir`
    /// puis recree le torrent sur ce dossier (rqbit ne peut pas
    /// reallouer un torrent vivant — remove/move/re-add, le hash-check
    /// au re-add reconnait les fichiers deplaces).
    ///
    /// `completed_dir` (`None` = `dest_dir`, comme Python) est
    /// applique seulement si le telechargement n'est pas termine ;
    /// sinon c'est `dest_dir` qui devient le `completed_dir`
    /// (comportement du handler Python).
    ///
    /// Retourne `false` pour le no-op Python (`dest_dir` inchange et
    /// `completed_dir` identique ou absent).
    pub async fn move_storage(
        &self,
        id_or_hash: &str,
        dest_dir: &Path,
        completed_dir: Option<&Path>,
    ) -> Result<bool> {
        let (dl, mut row) = self.download_and_row(id_or_hash)?;
        let current = dl.output_folder();
        let completed = completed_dir.unwrap_or(dest_dir);
        let cur_completed = row
            .completed_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_default();
        if dest_dir == current
            && completed_dir
                .map(|c| c == cur_completed.as_path())
                .unwrap_or(true)
        {
            return Ok(false);
        }
        if !dest_dir.is_dir() || !completed.is_dir() {
            return Err(CoreError::State(format!(
                "Target directory ({}) does not exist",
                dest_dir.display()
            )));
        }
        if row.torrent_data.is_none() {
            row.torrent_data = dl.torrent_bytes().map(|b| b.to_vec());
        }
        let finished = row.finished;
        let engine = self.engine_for(row.anon_hops.max(0) as u32).await?;
        // Les handles fichiers doivent etre fermes avant le
        // deplacement (rename impossible sinon sous Windows).
        self.remove_engine_only(id_or_hash, false).await?;
        if let Err(e) = move_dir_contents(&current, dest_dir) {
            // Rollback best-effort : remettre les entrees deja
            // deplacees puis re-add a l'ancien emplacement.
            let _ = move_dir_contents(dest_dir, &current);
            let _ = self.readd_row(&engine, &row).await;
            return Err(CoreError::State(format!(
                "move_storage: {e} (rollback effectue)"
            )));
        }
        row.output_dir = dest_dir.display().to_string();
        row.completed_dir = Some(
            if finished { dest_dir } else { completed }
                .display()
                .to_string(),
        );
        self.readd_row(&engine, &row).await?;
        self.inner
            .db
            .with(|c| tribler_db::downloads::upsert(c, &row))?;
        Ok(true)
    }

    /// `set_selected_files` Python : valide les indices puis applique
    /// `only_files` rqbit et persiste la selection.
    pub async fn set_selected_files(&self, id_or_hash: &str, files: &[i64]) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        let count = dl
            .file_count()
            .ok_or(CoreError::InvalidState("metainfo non disponible"))? as i64;
        if files.iter().any(|&i| i < 0 || i >= count) {
            return Err(CoreError::State("index out of range".into()));
        }
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let set: std::collections::HashSet<usize> = files.iter().map(|&i| i as usize).collect();
        engine.update_only_files(id_or_hash, &set).await?;
        self.update_download_row(&dl.info_hash(), |r| {
            r.selected_files = Some(files.to_vec());
        })?;
        Ok(())
    }

    /// `set_file_priority` Python — persistee pour reporting ;
    /// librqbit n'expose pas d'ordonnancement par priorite de
    /// fichier (ecart documente dans `api_rest_mapping.md`).
    pub fn set_file_priority(
        &self,
        id_or_hash: &str,
        file_index: i64,
        priority: i64,
    ) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        let count = dl
            .file_count()
            .ok_or(CoreError::InvalidState("metainfo non disponible"))? as i64;
        if !(0..count).contains(&file_index) {
            return Err(CoreError::State("index out of range".into()));
        }
        if !(0..=7).contains(&priority) {
            return Err(CoreError::State("file priority out of range".into()));
        }
        self.update_download_row(&dl.info_hash(), |r| {
            // Priorite 4 = normale libtorrent.
            let mut prios = r
                .file_priorities
                .take()
                .unwrap_or_else(|| vec![4; count as usize]);
            prios.resize(count as usize, 4);
            prios[file_index as usize] = priority;
            r.file_priorities = Some(prios);
        })?;
        Ok(())
    }

    /// Limites de debit par telechargement (octets/s, 0 = illimite),
    /// persistees et appliquees a la prochaine (re)creation —
    /// librqbit ne mute pas les limites d'un torrent vivant.
    pub fn set_rate_limits(
        &self,
        id_or_hash: &str,
        upload: Option<i64>,
        download: Option<i64>,
    ) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        self.update_download_row(&dl.info_hash(), |r| {
            if let Some(v) = upload {
                r.upload_limit = v;
            }
            if let Some(v) = download {
                r.download_limit = v;
            }
        })?;
        Ok(())
    }

    /// `set_seeding_ratio` Python — `None` reinitialise au defaut
    /// `download_defaults` (`seeding_ratio_default` du PATCH).
    pub fn set_seeding_ratio(&self, id_or_hash: &str, ratio: Option<f64>) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        self.update_download_row(&dl.info_hash(), |r| r.seeding_ratio = ratio)?;
        Ok(())
    }

    /// `set_auto_managed` Python — persiste (attribut logique ;
    /// librqbit n'a pas de file d'attente auto-managee).
    pub fn set_auto_managed(&self, id_or_hash: &str, enabled: bool) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        self.update_download_row(&dl.info_hash(), |r| r.auto_managed = enabled)?;
        Ok(())
    }

    /// `queue_position_{up,top,down,bottom}` Python : reordonne la
    /// colonne `queue_position` persistee de toutes les lignes
    /// (ordonnancement logique, expose dans le DTO).
    pub fn move_in_queue(&self, id_or_hash: &str, op: QueueOp) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        let ih = dl.info_hash();
        Ok(self.inner.db.with(|c| {
            let mut rows = tribler_db::downloads::list(c)?;
            rows.sort_by_key(|r| r.queue_position);
            let Some(pos) = rows.iter().position(|r| r.infohash == ih.to_vec()) else {
                return Ok(());
            };
            let last = rows.len().saturating_sub(1);
            let new_pos = match op {
                QueueOp::Up => pos.saturating_sub(1),
                QueueOp::Top => 0,
                QueueOp::Down => (pos + 1).min(last),
                QueueOp::Bottom => last,
            };
            let row = rows.remove(pos);
            rows.insert(new_pos, row);
            for (i, mut r) in rows.into_iter().enumerate() {
                r.queue_position = i as i64;
                tribler_db::downloads::upsert(c, &r)?;
            }
            Ok(())
        })?)
    }

    /// Marque l'intention pause/reprise dans la persistance
    /// (`user_stopped`/`paused` Python) — appele par le PATCH sur
    /// `state=stop|resume`.
    pub fn set_stopped_flag(&self, id_or_hash: &str, stopped: bool) -> Result<()> {
        let (dl, _) = self.download_and_row(id_or_hash)?;
        self.update_download_row(&dl.info_hash(), |r| {
            r.paused = stopped;
            r.user_stopped = stopped;
        })?;
        Ok(())
    }

    /// Demarre les services secondaires actives par la
    /// configuration (watch folder, RSS, torrent checker).
    async fn start_services(&self, config: &CoreConfig) {
        let mut services = Services::default();
        if let Some(dir) = &config.watch_folder_dir {
            let svc = crate::services::watch_folder::WatchFolderService::new(
                self.clone(),
                dir.clone(),
                std::time::Duration::from_millis(config.watch_folder_interval_ms),
            );
            svc.start();
            self.inner.asyncio.tasks.register(
                Some("WatchFolderService"),
                "check",
                Some(config.watch_folder_interval_ms as f64 / 1000.0),
            );
            services.watch_folder = Some(svc);
        }
        if !config.rss_urls.is_empty() {
            let mgr = crate::services::rss::RssManager::new(
                self.inner.notifier.clone(),
                config.ip_policy.clone(),
                self.inner.db.clone(),
                self.inner.asyncio.tasks.clone(),
            );
            mgr.update(&config.rss_urls);
            services.rss = Some(mgr);
        }
        if config.enable_torrent_checker {
            match crate::services::torrent_checker::TorrentChecker::new(
                self.inner.db.clone(),
                self.inner.notifier.clone(),
                config.ip_policy.clone(),
            )
            .await
            {
                Ok(checker) => {
                    let interval =
                        std::time::Duration::from_millis(config.torrent_checker_interval_ms);
                    self.inner.asyncio.tasks.register(
                        Some("TorrentChecker"),
                        "check_oldest",
                        Some(interval.as_secs_f64()),
                    );
                    let checker = Arc::new(checker);
                    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
                    let c = checker.clone();
                    tokio::spawn(async move {
                        let mut tick = tokio::time::interval(interval);
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                _ = stop_rx.changed() => break,
                                _ = tick.tick() => {
                                    let _ = c.check_oldest().await;
                                }
                            }
                        }
                    });
                    services.checker = Some(checker);
                    services.checker_stop = Some(stop_tx);
                }
                Err(e) => tracing::warn!(error = %e, "torrent checker indisponible"),
            }
        }
        *self.inner.services.lock().unwrap() = services;
    }

    /// Acces au torrent checker (API, tests).
    pub fn torrent_checker(&self) -> Option<Arc<crate::services::torrent_checker::TorrentChecker>> {
        self.inner.services.lock().unwrap().checker.clone()
    }

    /// Acces au gestionnaire RSS.
    pub fn rss(&self) -> Option<Arc<crate::services::rss::RssManager>> {
        self.inner.services.lock().unwrap().rss.clone()
    }

    /// Stack IPv8 de la session (`None` si `config.ipv8.enabled =
    /// false` — equivalent de `session.ipv8` conditionnel Python).
    pub fn ipv8(&self) -> Option<Arc<crate::ipv8_stack::Ipv8Stack>> {
        self.inner.ipv8.clone()
    }

    /// Moniteur `/api/ipv8/asyncio/*` (drift, taches, debug).
    pub fn asyncio(&self) -> &crate::asyncio::AsyncioMonitor {
        &self.inner.asyncio
    }

    /// `DHTDiscoveryCommunity` (`None` si IPv8 ou `dht_discovery` est
    /// desactive — `session.get_overlay(DHTCommunity)` Python rend
    /// alors `None` et le `dht_endpoint` repond 404).
    pub fn dht(&self) -> Option<Arc<tribler_ipv8::dht::DhtCommunity>> {
        self.inner.ipv8.as_ref().and_then(|s| s.dht.clone())
    }

    /// Acces a la base de metadonnees (endpoints `/api/metadata`).
    pub fn db(&self) -> &Arc<Database> {
        &self.inner.db
    }

    /// Configuration effective de la session.
    pub fn config(&self) -> &CoreConfig {
        &self.inner.config
    }

    /// Config de demarrage + overrides `apply_service_settings`
    /// (reglages effectivement en cours pour `GET /api/settings`).
    pub fn effective_config(&self) -> CoreConfig {
        let mut cfg = self.inner.config.clone();
        let ov = self.inner.overrides.read().unwrap();
        if let Some(urls) = &ov.rss_urls {
            cfg.rss_urls = urls.clone();
        }
        if let Some(dir) = &ov.watch_folder_dir {
            cfg.watch_folder_dir = dir.clone();
        }
        if let Some(dir) = &ov.download_dir {
            cfg.engine.output_dir = dir.clone();
        }
        if let Some(q) = &ov.queue {
            cfg.queue = q.clone();
        }
        if let Some((up, down)) = ov.rate_limits {
            cfg.engine.max_upload_bps = up;
            cfg.engine.max_download_bps = down;
        }
        cfg
    }

    /// Reconfigure les services a chaud (`POST /api/settings`) :
    /// URLs RSS, watch folder et dossier par defaut des telechargements.
    pub fn apply_service_settings(&self, config: &CoreConfig) {
        // Memorise le sous-ensemble applique pour que `effective_config()`
        // (et `GET /api/settings`) reflete le reglage courant.
        *self.inner.overrides.write().unwrap() = ServiceOverrides {
            rss_urls: Some(config.rss_urls.clone()),
            watch_folder_dir: Some(config.watch_folder_dir.clone()),
            download_dir: Some(config.engine.output_dir.clone()),
            queue: Some(config.queue.clone()),
            rate_limits: Some((config.engine.max_upload_bps, config.engine.max_download_bps)),
        };
        // `set_session_limits` Python : les bornes de debit de
        // session s'appliquent a chaud sur toutes les lanes
        // (`Session::ratelimits` rqbit est mutable). La file
        // (`active_*`) est relue par `enforce_queue_limits` au tick
        // suivant via `effective_config`.
        for engine in self.all_engines() {
            engine.set_ratelimits(config.engine.max_upload_bps, config.engine.max_download_bps);
        }
        let mut services = self.inner.services.lock().unwrap();
        // RSS : mise a jour du manager existant ou creation.
        if let Some(rss) = &services.rss {
            rss.update(&config.rss_urls);
        } else if !config.rss_urls.is_empty() {
            let mgr = crate::services::rss::RssManager::new(
                self.inner.notifier.clone(),
                config.ip_policy.clone(),
                self.inner.db.clone(),
                self.inner.asyncio.tasks.clone(),
            );
            mgr.update(&config.rss_urls);
            services.rss = Some(mgr);
        }
        // Watch folder : redemarrage si le repertoire change.
        let current = services
            .watch_folder
            .as_ref()
            .map(|w| w.directory().to_path_buf());
        if current != config.watch_folder_dir {
            if let Some(w) = services.watch_folder.take() {
                w.stop();
            }
            if let Some(dir) = &config.watch_folder_dir {
                let svc = crate::services::watch_folder::WatchFolderService::new(
                    self.clone(),
                    dir.clone(),
                    std::time::Duration::from_millis(config.watch_folder_interval_ms),
                );
                svc.start();
                services.watch_folder = Some(svc);
            }
        }
    }

    /// Sonde `low_space` : emet `Notification::LowSpace` quand
    /// l'espace libre du dossier de telechargement passe sous le seuil
    /// [`LOW_SPACE_THRESHOLD_BYTES`]. `disk_usage_data` reprend le
    /// format `shutil.disk_usage` du `statistics_endpoint` Python
    /// (`total`/`used`/`free` — Tribler 8.x ne re-emet plus ce topic,
    /// la sonde est une extension du daemon documentee).
    pub fn check_low_space(&self) {
        let dir = &self.inner.config.downloads_dir;
        let Ok(total) = fs2::total_space(dir) else {
            return;
        };
        let Ok(free) = fs2::available_space(dir) else {
            return;
        };
        if free < LOW_SPACE_THRESHOLD_BYTES {
            self.inner.notifier.notify(Notification::LowSpace {
                disk_usage_data: serde_json::json!({
                    "total": total,
                    "used": total.saturating_sub(free),
                    "free": free,
                }),
            });
        }
    }

    /// Cle publique IPv8 de la session (hex) — `events_start` /
    /// `/api/events/info` ; vide si IPv8 est desactive.
    pub fn public_key_hex(&self) -> String {
        self.inner
            .ipv8
            .as_ref()
            .map(|s| s.public_key_hex())
            .unwrap_or_default()
    }

    /// `tribler_shutdown_state` : etape de la sequence d'arret
    /// (messages de `Session.shutdown()` Python).
    fn shutdown_state(&self, state: &str) {
        self.inner.notifier.notify(Notification::ShutdownState {
            state: state.to_string(),
        });
    }

    /// Arret propre : services, moteur puis notification. Idempotent —
    /// les appels concurrents ou répétés (tray, API, Ctrl-C) sont des
    /// no-ops.
    pub async fn stop(&self) {
        if self
            .inner
            .stopped
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        self.inner.notifier.notify(Notification::SessionStopping);
        let services = std::mem::take(&mut *self.inner.services.lock().unwrap());
        self.shutdown_state("Shutting down torrent checker.");
        if let Some(w) = &services.watch_folder {
            w.stop();
        }
        if let Some(r) = &services.rss {
            r.stop();
        }
        if let Some(tx) = &services.checker_stop {
            let _ = tx.send(true);
        }
        // Arret des lanes anonymes puis de la stack IPv8 (overlays +
        // interface SOCKS5 locale des lanes).
        self.shutdown_state("Shutting down IPv8 peer-to-peer overlays.");
        if let Some(stack) = &self.inner.ipv8 {
            stack.stop().await;
        }
        self.shutdown_state("Shutting down download manager.");
        self.inner.engine.stop().await;
        self.shutdown_state("Shutting down local SOCKS5 interface.");
        self.shutdown_state("Shutting down metadata database.");
        self.shutdown_state("Shutting down GUI connection. Going dark.");
    }
}

/// Deplace les entrees de premier niveau de `src` vers `dst`
/// (`move_storage` : `rename` intra-volume ; copie + suppression en
/// secours pour les deplacements inter-volumes).
fn move_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if to.exists() {
            // Collision : la destination prevaut — les pieces non
            // deplacees seront reverifiees au re-add.
            continue;
        }
        if std::fs::rename(&from, &to).is_err() {
            copy_recursive(&from, &to)?;
            if from.is_dir() {
                std::fs::remove_dir_all(&from)?;
            } else {
                std::fs::remove_file(&from)?;
            }
        }
    }
    Ok(())
}

/// Copie recursive fichier/repertoire (secours inter-volumes de
/// [`move_dir_contents`]).
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Demarre la stack IPv8 si `config.ipv8.enabled` (endpoint UDP,
/// discovery, content discovery, tunnel + lanes anonymes).
async fn start_ipv8(
    config: &CoreConfig,
    db: Arc<Database>,
    notifier: Notifier,
    tasks: crate::asyncio::TaskRegistry,
) -> Result<Option<Arc<crate::ipv8_stack::Ipv8Stack>>> {
    if !config.ipv8.enabled {
        return Ok(None);
    }
    let stack = crate::ipv8_stack::Ipv8Stack::start(
        &config.ipv8,
        &config.state_dir,
        &config.downloads_dir,
        &config.engine,
        db,
        notifier,
        tasks,
    )
    .await?;
    Ok(Some(stack))
}
