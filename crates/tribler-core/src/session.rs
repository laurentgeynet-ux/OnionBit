//! `CoreSession` : orchestration du domaine.
//!
//! Equivalent de `tribler.core.session.Session` : assemble les ports
//! d'infrastructure (moteur BitTorrent, base) derriere une facade
//! coherente, publie les evenements sur le [`Notifier`], et restaure
//! les telechargements connus au demarrage.

use std::sync::Arc;

use tribler_bittorrent::{BtEngine, Download, DownloadStats};
use tribler_db::{Database, DownloadRow};

use crate::config::CoreConfig;
use crate::error::{CoreError, Result};
use crate::notifier::{Notification, Notifier};

/// Unite de temps des timestamps persistants : secondes Unix.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    /// restaure les telechargements connus, lance la boucle de
    /// publication de progression.
    pub async fn start(config: CoreConfig, notifier: Notifier) -> Result<Self> {
        std::fs::create_dir_all(&config.state_dir)?;
        let db = Database::open(&config.db_path())?;
        let engine = BtEngine::start(config.engine.clone()).await?;
        let db = Arc::new(db);
        let ipv8 = start_ipv8(&config, db.clone()).await?;
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
            }),
        };
        session.start_services(&services_config).await;
        session.restore_downloads().await;
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
        let ipv8 = start_ipv8(&config, db.clone()).await?;
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
            }),
        };
        session.start_services(&services_config).await;
        session.spawn_progress_loop();
        Ok(session)
    }

    /// Reinjecte dans les moteurs les telechargements persistes
    /// (le moteur anonyme `anon_hops` est choisi selon la colonne DB).
    async fn restore_downloads(&self) {
        let rows = match self.inner.db.with(tribler_db::downloads::list) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "lecture des downloads persistes impossible");
                return;
            }
        };
        for row in rows {
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
            let res = if let Some(data) = &row.torrent_data {
                engine.add_torrent_bytes(data.clone(), row.paused).await
            } else {
                engine.add_uri(&row.source_uri).await
            };
            match res {
                Ok(dl) => {
                    tracing::info!(
                        infohash = %dl.info_hash_hex(),
                        "telechargement restaure"
                    );
                    if row.paused {
                        let _ = engine.pause(&dl.id().to_string()).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        infohash = %hex::encode(&row.infohash),
                        error = %e,
                        "restauration d'un telechargement echouee"
                    );
                }
            }
        }
    }

    /// Boucle periodique : publie `DownloadProgress` pour chaque
    /// telechargement actif et detecte les fins de telechargement.
    fn spawn_progress_loop(&self) {
        let session = self.clone();
        let interval = std::time::Duration::from_millis(self.inner.config.progress_interval_ms);
        tokio::spawn(async move {
            let mut finished: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                for stats in session.inner.engine.list() {
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
                        }
                    }
                    session
                        .inner
                        .notifier
                        .notify(Notification::DownloadProgress(stats));
                }
            }
        });
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

    /// Ajoute un telechargement (magnet ou URI `http(s)`), eventuellement
    /// anonyme (`anon_hops` sauts de tunnel — necessite
    /// `ipv8.enable_anonymity`), et le persiste.
    pub async fn add_download(&self, uri: &str, paused: bool) -> Result<Download> {
        self.add_download_anon(uri, paused, 0).await
    }

    /// `add_download` avec choix du nombre de sauts anonymes.
    pub async fn add_download_anon(
        &self,
        uri: &str,
        paused: bool,
        anon_hops: u32,
    ) -> Result<Download> {
        if anon_hops == 0 {
            self.check_uri_policy(uri).await?;
        }
        let engine = self.engine_for(anon_hops).await?;
        let dl = engine.add_uri(uri).await?;
        self.persist(&dl, uri, paused, anon_hops)?;
        Ok(dl)
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
        self.add_torrent_bytes_anon(bytes, paused, 0).await
    }

    /// `add_torrent_bytes` avec choix du nombre de sauts anonymes.
    pub async fn add_torrent_bytes_anon(
        &self,
        bytes: Vec<u8>,
        paused: bool,
        anon_hops: u32,
    ) -> Result<Download> {
        // Parsing borne en amont pour extraire l'info-hash a persister.
        let meta = tribler_format::torrent::TorrentMeta::parse(&bytes)?;
        let engine = self.engine_for(anon_hops).await?;
        let dl = engine.add_torrent_bytes(bytes.clone(), paused).await?;
        self.persist_torrent(&dl, bytes, &meta, paused, anon_hops)?;
        Ok(dl)
    }

    fn persist(&self, dl: &Download, uri: &str, paused: bool, anon_hops: u32) -> Result<()> {
        self.inner.db.with(|c| {
            tribler_db::downloads::upsert(
                c,
                &DownloadRow {
                    infohash: dl.info_hash().to_vec(),
                    name: dl.name(),
                    source_uri: uri.to_string(),
                    output_dir: dl.output_folder().display().to_string(),
                    added_on: now_unix(),
                    paused,
                    anon_hops: anon_hops as i64,
                    ..Default::default()
                },
            )
        })?;
        Ok(())
    }

    fn persist_torrent(
        &self,
        dl: &Download,
        bytes: Vec<u8>,
        meta: &tribler_format::torrent::TorrentMeta,
        paused: bool,
        anon_hops: u32,
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
                    paused,
                    anon_hops: anon_hops as i64,
                    ..Default::default()
                },
            )
        })?;
        Ok(())
    }

    /// `update_hops` Python (`DownloadManager.update_hops`) : retire le
    /// telechargement de son moteur actuel puis le recree sur le moteur
    /// a `new_hops` sauts — les donnees sur disque sont conservees et
    /// le telechargement repart actif (comportement Python identique).
    ///
    /// La source de re-creation est, par ordre : les octets du
    /// `.torrent` persistes en base, sinon la `source_uri` enregistree.
    pub async fn update_hops(&self, id_or_hash: &str, new_hops: u32) -> Result<()> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let ih = dl.info_hash();
        let row = self.inner.db.with(|c| tribler_db::downloads::get(c, &ih))?;
        let old_hops = row.as_ref().map(|r| r.anon_hops.max(0) as u32).unwrap_or(0);
        let bytes = row
            .as_ref()
            .and_then(|r| r.torrent_data.clone())
            .or_else(|| dl.torrent_bytes().map(|b| b.to_vec()));
        let uri = row
            .map(|r| r.source_uri)
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| {
                format!("magnet:?xt=urn:btih:{}", tribler_crypto::hash::to_hex(&ih))
            });
        self.remove(id_or_hash, false).await?;
        let readded = match bytes.clone() {
            Some(b) => self.add_torrent_bytes_anon(b, false, new_hops).await,
            None => self.add_download_anon(&uri, false, new_hops).await,
        };
        if let Err(e) = readded {
            // Rollback best-effort sur l'ancien moteur : Python laisse
            // le download perdu dans ce cas, mais la ligne DB disparait
            // aussi chez nous — on prefere restaurer l'etat initial.
            let rollback = match bytes {
                Some(b) => self.add_torrent_bytes_anon(b, false, old_hops).await,
                None => self.add_download_anon(&uri, false, old_hops).await,
            };
            if let Err(rb) = rollback {
                tracing::warn!(
                    infohash = %tribler_crypto::hash::to_hex(&ih),
                    error = %rb,
                    "update_hops: rollback impossible — download perdu"
                );
            }
            return Err(e);
        }
        Ok(())
    }

    /// Liste les telechargements (moteur principal + lanes anonymes).
    pub fn downloads(&self) -> Vec<DownloadStats> {
        self.all_engines().iter().flat_map(|e| e.list()).collect()
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

    /// Pause / reprise / suppression.
    pub async fn pause(&self, id_or_hash: &str) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        engine.pause(id_or_hash).await?;
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Reprend un telechargement.
    pub async fn resume(&self, id_or_hash: &str) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        engine.resume(id_or_hash).await?;
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Supprime un telechargement (optionnellement ses fichiers).
    pub async fn remove(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let engine = self
            .owner_engine(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let infohash = engine.get(id_or_hash).map(|d| d.info_hash_hex());
        engine.remove(id_or_hash, delete_files).await?;
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
            services.watch_folder = Some(svc);
        }
        if !config.rss_urls.is_empty() {
            let mgr = crate::services::rss::RssManager::new(
                self.inner.notifier.clone(),
                config.ip_policy.clone(),
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
        cfg
    }

    /// Reconfigure les services a chaud (`POST /api/settings`) :
    /// URLs RSS et watch folder. Les autres champs de config sont
    /// consultables via `config()` mais non mutables a chaud.
    pub fn apply_service_settings(&self, config: &CoreConfig) {
        // Memorise le sous-ensemble applique pour que `effective_config()`
        // (et `GET /api/settings`) reflete le reglage courant.
        *self.inner.overrides.write().unwrap() = ServiceOverrides {
            rss_urls: Some(config.rss_urls.clone()),
            watch_folder_dir: Some(config.watch_folder_dir.clone()),
        };
        let mut services = self.inner.services.lock().unwrap();
        // RSS : mise a jour du manager existant ou creation.
        if let Some(rss) = &services.rss {
            rss.update(&config.rss_urls);
        } else if !config.rss_urls.is_empty() {
            let mgr = crate::services::rss::RssManager::new(
                self.inner.notifier.clone(),
                config.ip_policy.clone(),
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

    /// Arret propre : services, moteur puis notification.
    pub async fn stop(&self) {
        self.inner.notifier.notify(Notification::SessionStopping);
        let services = std::mem::take(&mut *self.inner.services.lock().unwrap());
        if let Some(w) = &services.watch_folder {
            w.stop();
        }
        if let Some(r) = &services.rss {
            r.stop();
        }
        if let Some(tx) = &services.checker_stop {
            let _ = tx.send(true);
        }
        // Arret des lanes anonymes puis de la stack IPv8.
        if let Some(stack) = &self.inner.ipv8 {
            stack.stop().await;
        }
        self.inner.engine.stop().await;
    }
}

/// Demarre la stack IPv8 si `config.ipv8.enabled` (endpoint UDP,
/// discovery, content discovery, tunnel + lanes anonymes).
async fn start_ipv8(
    config: &CoreConfig,
    db: Arc<Database>,
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
    )
    .await?;
    Ok(Some(stack))
}
