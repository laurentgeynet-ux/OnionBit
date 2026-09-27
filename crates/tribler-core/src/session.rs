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

struct Inner {
    config: CoreConfig,
    engine: BtEngine,
    db: Database,
    notifier: Notifier,
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
        let session = Self {
            inner: Arc::new(Inner {
                config,
                engine,
                db,
                notifier,
            }),
        };
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
        let session = Self {
            inner: Arc::new(Inner {
                config,
                engine,
                db,
                notifier,
            }),
        };
        session.spawn_progress_loop();
        Ok(session)
    }

    /// Reinjecte dans le moteur les telechargements persistes.
    async fn restore_downloads(&self) {
        let rows = match self.inner.db.with(tribler_db::downloads::list) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "lecture des downloads persistes impossible");
                return;
            }
        };
        for row in rows {
            let res = if let Some(data) = &row.torrent_data {
                self.inner
                    .engine
                    .add_torrent_bytes(data.clone(), row.paused)
                    .await
            } else {
                self.inner.engine.add_uri(&row.source_uri).await
            };
            match res {
                Ok(dl) => {
                    tracing::info!(
                        infohash = %dl.info_hash_hex(),
                        "telechargement restaure"
                    );
                    if row.paused {
                        let _ = self.inner.engine.pause(&dl.id().to_string()).await;
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

    /// Ajoute un telechargement (magnet ou URI `http(s)`) et le
    /// persiste.
    pub async fn add_download(&self, uri: &str, paused: bool) -> Result<Download> {
        self.check_uri_policy(uri).await?;
        let dl = self.inner.engine.add_uri(uri).await?;
        self.persist(&dl, uri, paused)?;
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
        // Parsing borne en amont pour extraire l'info-hash a persister.
        let meta = tribler_format::torrent::TorrentMeta::parse(&bytes)?;
        let dl = self
            .inner
            .engine
            .add_torrent_bytes(bytes.clone(), paused)
            .await?;
        self.persist_torrent(&dl, bytes, &meta, paused)?;
        Ok(dl)
    }

    fn persist(&self, dl: &Download, uri: &str, paused: bool) -> Result<()> {
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
                    ..Default::default()
                },
            )
        })?;
        Ok(())
    }

    /// Liste les telechargements.
    pub fn downloads(&self) -> Vec<DownloadStats> {
        self.inner.engine.list()
    }

    /// Pause / reprise / suppression.
    pub async fn pause(&self, id_or_hash: &str) -> Result<()> {
        self.inner.engine.pause(id_or_hash).await?;
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Reprend un telechargement.
    pub async fn resume(&self, id_or_hash: &str) -> Result<()> {
        self.inner.engine.resume(id_or_hash).await?;
        self.notify_state(id_or_hash);
        Ok(())
    }

    /// Supprime un telechargement (optionnellement ses fichiers).
    pub async fn remove(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let infohash = self.inner.engine.get(id_or_hash).map(|d| d.info_hash_hex());
        self.inner.engine.remove(id_or_hash, delete_files).await?;
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
        if let Some(d) = self.inner.engine.get(id_or_hash) {
            let s = d.stats();
            self.inner
                .notifier
                .notify(Notification::DownloadStateChanged {
                    infohash: s.info_hash,
                    state: s.state,
                });
        }
    }

    /// Arret propre : moteur puis notification.
    pub async fn stop(&self) {
        self.inner.notifier.notify(Notification::SessionStopping);
        self.inner.engine.stop().await;
    }
}
