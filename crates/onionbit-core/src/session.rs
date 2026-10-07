// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CoreSession` : orchestration du domaine.
//!
//! Equivalent de `tribler.core.session.Session` : assemble les ports
//! d'infrastructure (moteur BitTorrent, base) derriere une facade
//! coherente, publie les evenements sur le [`Notifier`], et restaure
//! les telechargements connus au demarrage.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use onionbit_bittorrent::{AddDownloadOptions, BtEngine, Download, DownloadState, DownloadStats};
use onionbit_db::{Database, DownloadRow};

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
    /// Arret de la tache de mesure de capacite (`bandwidth`).
    bandwidth_stop: Option<tokio::sync::watch::Sender<bool>>,
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
    /// Defauts des nouveaux telechargements (`Some` = remplace
    /// `config.download_defaults`) — Python lit `download_defaults`
    /// depuis l'objet config mute in-place, donc a chaud.
    download_defaults: Option<crate::config::DownloadDefaults>,
    /// Sections restart-only memorisees telles que configurees au
    /// dernier `POST /api/settings` : elles ne sont PAS appliquees a
    /// la session en cours (comme Python, qui ne relit ces cles qu'au
    /// demarrage des composants), mais `effective_config()` — donc
    /// `GET /api/settings` — doit refleter la valeur en attente de
    /// redemarrage plutot que l'etat de demarrage (sinon les
    /// commutateurs de l'UI reviennent a leur position initiale).
    ipv8: Option<crate::ipv8_stack::Ipv8Config>,
    /// Idem pour la section moteur (`dht`/`utp`/`proxy_*`/ports…).
    engine: Option<onionbit_bittorrent::EngineConfig>,
    /// Idem pour `torrent_checker/enabled`.
    enable_torrent_checker: Option<bool>,
}

/// Parametres initiaux communs de `persist`/`persist_torrent`.
struct PersistParams {
    paused: bool,
    anon_hops: u32,
    safe_seeding: bool,
    extra_trackers: Vec<String>,
}

/// Ajout magnet en cours de resolution des metadonnees (BEP 9) —
/// librqbit ne cree le torrent qu'apres `resolve_magnet` ; Python
/// affiche le download immediatement en etat `METADATA`, on expose
/// donc l'attente via `pending_downloads()` + `GET /api/downloads`.
#[derive(Debug, Clone)]
pub struct PendingDownload {
    /// Info-hash hex (v1).
    pub infohash: String,
    /// Nom d'affichage (`dn` du magnet).
    pub name: Option<String>,
    /// Sauts anonymes demandes.
    pub anon_hops: u32,
    /// Ajout en pause.
    pub paused: bool,
    /// Timestamp d'ajout (secondes Unix).
    pub added_on: i64,
}

/// Garde RAII d'une entree `pending` magnet : un add abandonne en
/// vol — le futur est drope au milieu de `resolve_magnet` (timeout ou
/// annulation cote appelant, requete HTTP coupee) — retire l'entree,
/// son `pending_notify` et le swarm enregistre. Sans cela l'infohash
/// restait `is_pending` definitivement : tout re-add ulterieur
/// echouait en `InvalidState("deja en cours de resolution")` et le
/// swarm `pending` fuyait jusqu'au prochain demarrage.
struct PendingAddGuard<'a> {
    inner: &'a Inner,
    key: Option<String>,
    infohash: Option<onionbit_crypto::hash::InfoHashV1>,
}

impl<'a> PendingAddGuard<'a> {
    fn new(
        inner: &'a Inner,
        key: String,
        infohash: Option<onionbit_crypto::hash::InfoHashV1>,
    ) -> Self {
        Self {
            inner,
            key: Some(key),
            infohash,
        }
    }

    /// Sortie normale (succes ou erreur) : le nettoyage explicite a
    /// deja ete fait — le `Drop` devient un no-op.
    fn disarm(&mut self) {
        self.key = None;
    }
}

impl Drop for PendingAddGuard<'_> {
    fn drop(&mut self) {
        let Some(k) = self.key.take() else {
            return;
        };
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&k);
        self.inner
            .pending_notify
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&k);
        if let (Some(stack), Some(ih)) = (self.inner.ipv8.as_ref(), self.infohash) {
            stack.clear_pending_swarm(&ih, false);
        }
    }
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
    /// Magnets en cours de resolution BEP 9 (cle : infohash hex) —
    /// visibles dans `GET /api/downloads` en statut `METADATA`.
    pending: std::sync::Mutex<std::collections::HashMap<String, PendingDownload>>,
    /// Signal de re-ciblage par infohash hex : `update_hops` sur une
    /// entree `pending` reveille la tache de resolution qui relance
    /// `add_uri_opts`/`readd_row` sur la nouvelle lane (la resolution
    /// BEP 9 en vol ne peut pas changer de lane — elle est abandonnee
    /// puis recreee).
    pending_notify:
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Notify>>>,
    /// Fin de la restauration des telechargements persistes
    /// (`load_checkpoint` Python — asynchrone) : `false` → `true`
    /// quand la tache de fond a termine ; [`Self::wait_restored`]
    /// permet aux tests de l'attendre.
    restore_done: tokio::sync::watch::Sender<bool>,
    /// `AugmentedSearch` Python : vocabulaire de sous-mots appris des
    /// titres de torrents, utilise par `local_search` (`augmenter`
    /// du `DatabaseEndpoint`).
    augmenter: Arc<crate::augmenter::Augmenter>,
    /// Estimateur de capacite upload (`tunnel_community/bandwidth`) —
    /// regle `max_relayed_rate` en mode auto (AIMD sur le retard de
    /// file) ; consultable via `/api/statistics/ipv8` (`bandwidth`).
    bandwidth: Arc<crate::services::bandwidth::CongestionController>,
    /// Horodatage de demarrage — expose via `uptime_secs`
    /// (`GET /api/statistics/tribler`).
    started_at: std::time::Instant,
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
        let augmenter = Arc::new(crate::augmenter::Augmenter::new(&config.state_dir));
        let notifier_aug = notifier.clone();
        let session = Self {
            inner: Arc::new(Inner {
                config,
                overrides: std::sync::RwLock::new(ServiceOverrides::default()),
                engine,
                db: db.clone(),
                notifier,
                services: std::sync::Mutex::new(Services::default()),
                ipv8,
                last_tracker_sync: std::sync::Mutex::new(None),
                asyncio,
                pending: std::sync::Mutex::new(std::collections::HashMap::new()),
                pending_notify: std::sync::Mutex::new(std::collections::HashMap::new()),
                stopped: std::sync::atomic::AtomicBool::new(false),
                restore_done: tokio::sync::watch::channel(false).0,
                augmenter: augmenter.clone(),
                bandwidth: Arc::new(crate::services::bandwidth::CongestionController::new()),
                started_at: std::time::Instant::now(),
            }),
        };
        // `AugmentedSearch` Python : abonne aux metadonnees ajoutees
        // et amorce le vocabulaire depuis la base s'il est vide
        // (`seed_augmenter` + `schedule_study`).
        augmenter.spawn_consumer(notifier_aug);
        if augmenter.needs_kickstart() {
            let titles = db
                .with(|c| {
                    let mut stmt = c.prepare(
                        "SELECT title FROM channel_node WHERE title != ''
                         ORDER BY RANDOM() LIMIT 10000",
                    )?;
                    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
                })
                .unwrap_or_default();
            augmenter.seed(titles);
            let aug = augmenter.clone();
            tokio::task::spawn_blocking(move || aug.study_pending());
        }
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
        let augmenter = Arc::new(crate::augmenter::Augmenter::new(&config.state_dir));
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
                pending: std::sync::Mutex::new(std::collections::HashMap::new()),
                pending_notify: std::sync::Mutex::new(std::collections::HashMap::new()),
                stopped: std::sync::atomic::AtomicBool::new(false),
                // Base memoire : rien a restaurer — restauration
                // marquee terminee d'emblee.
                restore_done: tokio::sync::watch::channel(true).0,
                augmenter,
                bandwidth: Arc::new(crate::services::bandwidth::CongestionController::new()),
                started_at: std::time::Instant::now(),
            }),
        };
        session
            .inner
            .augmenter
            .spawn_consumer(session.inner.notifier.clone());
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
            // `send_replace` et non `send` : le receveur initial du
            // canal est detruit a la construction et `send` echoue
            // sans receveur actif — une restauration plus rapide que
            // le premier `wait_restored()` perdrait alors le signal et
            // bloquerait l'attente pour toujours (base vide : chemin
            // le plus rapide).
            session.inner.restore_done.send_replace(true);
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

    /// `true` quand la restauration des telechargements persistes
    /// (`load_checkpoint` Python) est close — succes ou echecs
    /// partiels. Les lignes `downloads` jamais reinjectees dans un
    /// moteur ne doivent plus etre presentees comme « en file pour le
    /// check » une fois ce point passe : elles ne reviendront pas
    /// avant le prochain run.
    pub fn restore_finished(&self) -> bool {
        *self.inner.restore_done.borrow()
    }

    /// Reinjecte dans les moteurs les telechargements persistes, avec
    /// tous leurs reglages (equivalent du `load_checkpoint` Python :
    /// selection de fichiers, trackers additionnels, limites,
    /// dossier de sortie et etat pause sont reappliques a l'ajout).
    async fn restore_downloads(&self) {
        let t0 = std::time::Instant::now();
        let mut restored = 0usize;
        let mut failed = 0usize;
        let mut deferred = 0usize;
        let rows = match self.inner.db.with(onionbit_db::downloads::list) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "lecture des downloads persistes impossible");
                return;
            }
        };
        // Lanes anonymes deja temporisees : l'attente du premier
        // circuit `DATA` pret se fait une fois par `anon_hops`, les
        // downloads suivants de la meme lane passent directement.
        let mut awaited_lanes: std::collections::HashSet<u32> = std::collections::HashSet::new();
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
                    failed += 1;
                    tracing::warn!(
                        infohash = %hex::encode(&row.infohash),
                        error = %e,
                        "moteur anonyme indisponible a la restauration"
                    );
                    // `on_tribler_exception` Python : sans remontee au
                    // GUI le download resterait affiche « en
                    // verification » toute la session (ligne DB non
                    // reinjectee) sans explication visible.
                    self.inner.notifier.notify(Notification::TriblerException {
                        error: format!("restore {}: {e}", hex::encode(&row.infohash)),
                    });
                    continue;
                }
            };
            // Un download anonyme reajoute avant le premier circuit
            // `DATA` pret de sa lane voit tous ses envois (dial uTP,
            // trackers, DHT) tomber sur `select_circuit` (« aucun
            // circuit pret ») — librqbit peut alors rester dormant
            // jusqu'a un pause/reprise manuel. Attente bornee
            // (`next_hop_timeout`) et best-effort : a l'echeance le
            // re-add se fait quand meme (le Python restaure aussi
            // sans garantie de circuit).
            if row.anon_hops > 0 && awaited_lanes.insert(row.anon_hops as u32) {
                if let Some(tunnel) = self.inner.ipv8.as_ref().and_then(|s| s.tunnel.as_ref()) {
                    let ok = tunnel
                        .await_data_circuit_of_hops(
                            row.anon_hops as usize,
                            tunnel.settings.next_hop_timeout,
                        )
                        .await;
                    if !ok {
                        tracing::warn!(
                            anon_hops = row.anon_hops,
                            "aucun circuit pret sous next_hop_timeout — restauration poursuivie"
                        );
                    }
                }
            }
            if self.inner.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("restauration interrompue par l'arret de la session");
                return;
            }
            if row.torrent_data.is_none() {
                // Sans metainfo persistee (lignes laissees par un ajout
                // magnet/URI des versions anterieures), `readd_row`
                // attend la resolution BEP 9 ou le fetch HTTP de la
                // source — potentiellement jamais sur une lane
                // anonyme sans pair joignable, ce qui figeait la file
                // : les lignes suivantes restaient « en verification »
                // et tout PATCH repondait 404. Python cree le
                // Download d'emblee (etat METADATA) — on deporte le
                // re-add en tache de fond exposee via `pending`.
                self.spawn_deferred_restore(row);
                deferred += 1;
                continue;
            }
            match self.readd_row(&engine, &row).await {
                Ok(dl) => {
                    restored += 1;
                    tracing::info!(
                        infohash = %dl.info_hash_hex(),
                        "telechargement restaure"
                    );
                    if let Some(name) = dl.name() {
                        self.index_channel_node(&dl.info_hash(), &name, dl.stats().total_bytes);
                    }
                }
                Err(e) => {
                    failed += 1;
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
        // Recap de fin de restauration : le temps mur jusqu'ici est la
        // mesure pertinente (les phases rqbit instrumentees peuvent se
        // chevaucher entre torrents — la somme des elapsed_ms n'est pas
        // le temps ressenti).
        tracing::info!(
            restored,
            failed,
            deferred,
            total_elapsed_ms = t0.elapsed().as_millis() as u64,
            "restauration des telechargements terminee"
        );
    }

    /// Re-add deporte d'une ligne sans metainfo persistee : la ligne
    /// apparait en `pending` (statut METADATA) pendant la resolution
    /// de la source, puis le metainfo resolu est backfille dans
    /// `torrent_data` — les demarrages suivants passent alors par la
    /// voie `.torrent` synchrone.
    ///
    /// La lane cible est relue a chaque tentative : un `PATCH
    /// anon_hops` pendant la resolution abandonne l'attente en vol et
    /// relance sur la nouvelle lane (`pending_notify`). Retirer
    /// l'entree `pending` (DELETE) abandonne la restauration.
    fn spawn_deferred_restore(&self, row: DownloadRow) {
        let ih_hex = onionbit_crypto::hash::to_hex(&row.infohash);
        let notify = self.pending_notify(&ih_hex);
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                ih_hex.clone(),
                PendingDownload {
                    infohash: ih_hex.clone(),
                    name: row.name.clone(),
                    anon_hops: row.anon_hops.max(0) as u32,
                    paused: row.paused || row.user_stopped,
                    added_on: row.added_on,
                },
            );
        let session = self.clone();
        tokio::spawn(async move {
            let mut notified = std::pin::pin!(notify.notified());
            // Lane/pause effectivement tentees a la derniere
            // iteration — convergence a la materialisation.
            let mut used_hops = row.anon_hops.max(0) as u32;
            let mut used_paused = row.paused || row.user_stopped;
            let res = loop {
                notified.as_mut().enable();
                let Some((hops, paused)) = session
                    .inner
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&ih_hex)
                    .map(|p| (p.anon_hops, p.paused))
                else {
                    // Entree retiree entre-temps : suppression
                    // utilisateur, rien a restaurer ni a signaler.
                    break Err(CoreError::Cancelled("restauration abandonnee"));
                };
                used_hops = hops;
                used_paused = paused;
                let engine = match session.engine_for(hops).await {
                    Ok(e) => e,
                    Err(e) => break Err(e),
                };
                // `paused`/`user_stopped` ont pu changer pendant la
                // resolution (`PATCH state`) — options relues.
                let mut row = row.clone();
                row.paused = paused;
                row.user_stopped = paused;
                // Meme garde que `restore_downloads` : resolution BEP 9
                // et premiers dials passent par la lane — attendre le
                // premier circuit pret evite les envois droppes sur
                // `select_circuit`. Best-effort, borne `next_hop_timeout`.
                if hops > 0 {
                    if let Some(tunnel) =
                        session.inner.ipv8.as_ref().and_then(|s| s.tunnel.as_ref())
                    {
                        tunnel
                            .await_data_circuit_of_hops(
                                hops as usize,
                                tunnel.settings.next_hop_timeout,
                            )
                            .await;
                    }
                }
                let fut = session.readd_row(&engine, &row);
                tokio::pin!(fut);
                tokio::select! {
                    r = &mut fut => break r,
                    _ = notified.as_mut() => {
                        notified.set(notify.notified());
                        tracing::info!(
                            infohash = %ih_hex,
                            hops,
                            "restauration differee relancee sur la nouvelle lane"
                        );
                    }
                }
            };
            let removed = session
                .inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&ih_hex);
            session
                .inner
                .pending_notify
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&ih_hex);
            match res {
                Ok(dl) => {
                    // `update_download_row` renvoie faux quand la
                    // ligne a ete supprimee pendant la resolution :
                    // pas de download orphelin dans le moteur (idem
                    // apres `stop()`).
                    let known = session
                        .update_download_row(&dl.info_hash(), |r| {
                            if r.torrent_data.is_none() {
                                r.torrent_data = dl.torrent_bytes().map(|b| b.to_vec());
                            }
                            if r.name.is_none() {
                                r.name = dl.name();
                            }
                        })
                        .unwrap_or(false);
                    if !known
                        || session
                            .inner
                            .stopped
                            .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        let _ = session.remove_engine_only(&dl.info_hash_hex(), false).await;
                    } else {
                        // La cible `pending` a pu changer entre la
                        // derniere relance et la materialisation :
                        // le download etant desormais actif, le chemin
                        // normal migre la lane et applique la pause.
                        if let Some(p) = removed {
                            if p.anon_hops != used_hops {
                                if let Err(e) = session.update_hops(&ih_hex, p.anon_hops).await {
                                    tracing::warn!(
                                        infohash = %ih_hex,
                                        error = %e,
                                        "lane demandee pendant la resolution non appliquee"
                                    );
                                }
                            }
                            if p.paused != used_paused {
                                let r = if p.paused {
                                    session.pause(&ih_hex).await
                                } else {
                                    session.resume(&ih_hex).await
                                };
                                if let Err(e) = r {
                                    tracing::warn!(
                                        infohash = %ih_hex,
                                        error = %e,
                                        "etat pause demande pendant la resolution non applique"
                                    );
                                }
                            }
                        }
                        tracing::info!(
                            infohash = %ih_hex,
                            "telechargement restaure (resolution differee)"
                        );
                        if let Some(name) = dl.name() {
                            session.index_channel_node(
                                &dl.info_hash(),
                                &name,
                                dl.stats().total_bytes,
                            );
                        }
                    }
                }
                Err(CoreError::Cancelled(m)) => {
                    tracing::debug!(infohash = %ih_hex, "restauration differee: {m}");
                }
                Err(e) => {
                    tracing::warn!(
                        infohash = %ih_hex,
                        error = %e,
                        "restauration differee d'un telechargement echouee"
                    );
                    session
                        .inner
                        .notifier
                        .notify(Notification::TriblerException {
                            error: format!("restore {ih_hex}: {e}"),
                        });
                }
            }
        });
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
            // `output_dir` persiste = `Download::output_folder()` :
            // dossier final, nom du torrent deja inclus.
            output_includes_name: true,
            only_files: row
                .selected_files
                .as_ref()
                .map(|l| l.iter().map(|&i| i as usize).collect()),
            trackers: crate::trackers::effective_trackers(row),
            upload_limit_bps: u64::try_from(row.upload_limit).ok().filter(|&v| v > 0),
            download_limit_bps: u64::try_from(row.download_limit).ok().filter(|&v| v > 0),
            // Ephemere : pairs d'amorce d'ajout, non persistes.
            initial_peers: Vec::new(),
            // Ephemere aussi : le flux de pairs pending est recree
            // par la boucle de resolution magnet si besoin.
            extra_peers_rx: None,
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
        Ok(self.inner.db.with(|c| onionbit_db::downloads::get(c, ih))?)
    }

    /// Lecture-modification-ecriture des reglages persistes d'un
    /// telechargement (`true` si la ligne existait). Les colonnes
    /// runtime de la ligne relue sont conservees — `f` ne touche que
    /// les reglages.
    pub fn update_download_row(&self, ih: &[u8], f: impl FnOnce(&mut DownloadRow)) -> Result<bool> {
        Ok(self.inner.db.with(|c| {
            let Some(mut row) = onionbit_db::downloads::get(c, ih)? else {
                return Ok(false);
            };
            f(&mut row);
            onionbit_db::downloads::upsert(c, &row)?;
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
            // Pre-amorcage depuis le drapeau persistant `finished` : un
            // telechargement restaure deja termine n'est pas une
            // completion de cette session — l'alerte `torrent_finished`
            // de libtorrent ne se rejoue pas au chargement d'un
            // checkpoint. Sans cela, chaque demarrage re-notifiait tous
            // les telechargements termines (et relancait leur recheck).
            let mut finished: std::collections::HashSet<String> = session
                .inner
                .db
                .with(|c| {
                    Ok(onionbit_db::downloads::list(c)?
                        .into_iter()
                        .filter(|r| r.finished)
                        .map(|r| onionbit_crypto::hash::to_hex(&r.infohash))
                        .collect())
                })
                .unwrap_or_default();
            let mut queue_paused: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut backed_up: std::collections::HashSet<String> = std::collections::HashSet::new();
            // Derniere lecture des compteurs de session librqbit —
            // `uploaded_bytes`/`progress_bytes` repartent a zero au
            // (re-)add moteur : l'accumulation par delta les convertit
            // en cumuls tous-temps (`total_uploaded`/`total_downloaded`).
            let mut transferred: std::collections::HashMap<String, (u64, u64)> =
                std::collections::HashMap::new();
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                for stats in session.downloads() {
                    // `all_time_upload`/`all_time_download` Python :
                    // cumuls persistants par delta de session.
                    // Premiere vue : le delta vaut le compteur courant
                    // (toute la session jusqu'ici) ; re-add moteur
                    // (compteur revenu a 0) : `saturating_sub` evite
                    // de soustraire du cumul.
                    let (prev_up, prev_down) = transferred
                        .insert(
                            stats.info_hash.clone(),
                            (stats.uploaded_bytes, stats.progress_bytes),
                        )
                        .unwrap_or_default();
                    let du = stats.uploaded_bytes.saturating_sub(prev_up);
                    let dd = stats.progress_bytes.saturating_sub(prev_down);
                    if du > 0 || dd > 0 {
                        if let Some(ih) = onionbit_crypto::hash::from_hex(&stats.info_hash) {
                            let _ = session
                                .inner
                                .db
                                .with(|c| onionbit_db::downloads::add_transferred(c, &ih, du, dd));
                        }
                    }
                    if stats.finished {
                        // `insert` = passage a termine observe dans
                        // cette session : notification + drapeau
                        // persistant + recheck optionnel.
                        if finished.insert(stats.info_hash.clone()) {
                            session
                                .inner
                                .notifier
                                .notify(Notification::DownloadFinished {
                                    infohash: stats.info_hash.clone(),
                                    name: stats.name.clone(),
                                });
                            let ih = onionbit_crypto::hash::from_hex(&stats.info_hash);
                            if let Some(ih) = ih {
                                let _ = session
                                    .inner
                                    .db
                                    .with(|c| onionbit_db::downloads::set_finished(c, &ih, true));
                                // `add_download_to_channel` Python : les
                                // canaux ne sont pas portes — l'attribut
                                // persiste en base et le manque est trace.
                                let channel = session
                                    .inner
                                    .db
                                    .with(|c| onionbit_db::downloads::get(c, &ih))
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
                    } else if !matches!(
                        stats.state,
                        DownloadState::Initializing | DownloadState::Checking
                    ) && finished.remove(&stats.info_hash)
                    {
                        // Redevenu incomplet (selection de fichiers
                        // etendue, pieces invalidees) : le drapeau
                        // persistant suit pour que la prochaine
                        // completion notifie a nouveau. Les etats
                        // transitoires `Initializing`/`Checking` sont
                        // exclus : au demarrage, un telechargement
                        // termine restaure rapporte brievement
                        // `finished=false` pendant son hashcheck —
                        // retirer le drapeau ici redeclenchait
                        // `torrent_finished` a chaque boot.
                        if let Some(ih) = onionbit_crypto::hash::from_hex(&stats.info_hash) {
                            let _ = session
                                .inner
                                .db
                                .with(|c| onionbit_db::downloads::set_finished(c, &ih, false));
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
        let folder = self.download_defaults().torrent_folder;
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
        let path = Path::new(&folder).join(format!("{name} [{ih_hex}].torrent"));
        if let Err(e) = std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(&path, &bytes))
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

    /// Secondes depuis `start()` — `uptime_sec` de
    /// `GET /api/statistics/tribler`.
    pub fn uptime_secs(&self) -> u64 {
        self.inner.started_at.elapsed().as_secs()
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
        let ih = onionbit_crypto::hash::from_hex(hex)?;
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
        let Some(ih) = onionbit_crypto::hash::from_hex(&stats.info_hash) else {
            return;
        };
        let row = self
            .inner
            .db
            .with(|c| onionbit_db::downloads::get(c, &ih))
            .unwrap_or(None);
        let Some(row) = row else { return };
        if row.paused || row.user_stopped {
            return;
        }
        // Reglages lus a chaud : un changement de `seeding_mode` dans
        // `POST /api/settings` s'applique aux seeds existants au tick
        // suivant (Python relit `config` a chaque iteration).
        let dd = self.download_defaults();
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
            .with(onionbit_db::downloads::list)
            .unwrap_or_default();
        let row_of = |hash: &str| {
            onionbit_crypto::hash::from_hex(hash)
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
        let safe = self.download_defaults().safeseeding_enabled;
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
        self.add_download_anon_inner(
            uri,
            paused,
            anon_hops,
            safe_seeding,
            destination,
            Vec::new(),
        )
        .await
    }

    /// `add_download_anon` avec `initial_peers` rqbit injectes a
    /// l'ajout — utilise par les bancs live (seeder loopback annonce
    /// comme pair d'amorce, sans DHT ni trackers).
    pub async fn add_download_anon_with_peers(
        &self,
        uri: &str,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
        initial_peers: Vec<std::net::SocketAddr>,
    ) -> Result<Download> {
        self.add_download_anon_inner(
            uri,
            paused,
            anon_hops,
            safe_seeding,
            destination,
            initial_peers,
        )
        .await
    }

    async fn add_download_anon_inner(
        &self,
        uri: &str,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
        initial_peers: Vec<std::net::SocketAddr>,
    ) -> Result<Download> {
        if anon_hops == 0 {
            self.check_uri_policy(uri).await?;
        }
        self.check_low_space();
        // URI `http(s)` pointant un `.torrent` : le metainfo est
        // public — fetch en clair sous `ip_policy` (anti-SSRF,
        // timeout, taille bornee), puis ajout des octets par le
        // chemin fichier. Ne PAS deleguer le fetch a l'engine : sur
        // une lane anonyme, son client reqwest passe par le SOCKS5
        // du tunnel, dont `CONNECT` n'est pas un tunnel TCP — il ne
        // relaie qu'une requete HTTP claire one-shot (cellule
        // `http-request` vers une sortie `PEER_FLAG_EXIT_HTTP`), donc
        // TLS echoue et meme le HTTP clair exige un circuit deja
        // pret. Bonus : `private`/`removed_trackers` sont traites
        // comme pour un `.torrent` choisi en fichier.
        if uri.starts_with("http://") || uri.starts_with("https://") {
            let resp = crate::services::fetch_checked(uri, &self.inner.config.ip_policy).await?;
            let bytes = crate::services::read_body_limited(resp).await?;
            return self
                .add_torrent_bytes_anon_inner(
                    bytes.to_vec(),
                    paused,
                    anon_hops,
                    safe_seeding,
                    destination,
                    initial_peers,
                )
                .await;
        }
        // `download_exists` Python : un infohash deja gere (sur
        // n'importe quelle lane) n'est pas re-ajoute — sinon le
        // doublon ecrasait la ligne `downloads` partagee (anon_hops,
        // finished, reglages) du download existant.
        let magnet = onionbit_format::magnet::MagnetLink::parse(uri).ok();
        if let Some(m) = &magnet {
            let ih_hex = m.info_hash_hex();
            if let Some(existing) = self.find_download_hex(&ih_hex) {
                return Ok(existing);
            }
            // Dedup sur la resolution en vol (`download_exists`
            // Python couvre aussi l'etat METADATA) : sans cela un
            // second `PUT` ecrasait l'entree `pending` mais la
            // premiere tache de resolution continuait — deux
            // downloads moteur materialisaient le meme infohash
            // (lignes doublees, badge de lane de la derniere
            // persistance). Le `PATCH anon_hops` est la voie
            // prevue pour changer la lane d'un magnet en cours.
            if self.is_pending(&ih_hex) {
                return Err(CoreError::InvalidState(
                    "telechargement deja en cours de resolution",
                ));
            }
        }
        // Trackers par defaut (`trackers_file`) : ajoutes a chaque
        // nouveau telechargement, comme le post-handle
        // `ADD_DEFAULT_TRACKERS` Python — et persistes dans
        // `extra_trackers` pour survivre aux re-adds.
        let trackers = self.default_trackers().await;
        // Etat `METADATA` Python : la resolution BEP 9 d'un magnet
        // peut durer longtemps (surtout en lane anonyme) — le
        // download est visible dans `GET /api/downloads` pendant
        // l'attente au lieu de n'apparaitre qu'une fois resolu.
        let pending_key = magnet.as_ref().map(|m| m.info_hash_hex());
        // Garde RAII : un drop du futur en pleine resolution (timeout
        // ou annulation cote appelant) nettoie `pending` + swarm —
        // sinon l'infohash restait `is_pending` a vie et tout re-add
        // echouait en `InvalidState`.
        let mut pending_guard = pending_key.as_ref().map(|k| {
            PendingAddGuard::new(
                &self.inner,
                k.clone(),
                magnet.as_ref().and_then(|m| m.info_hash_v1),
            )
        });
        // Lane effectivement tentee a la derniere iteration — relue
        // depuis `pending` a chaque relance (`PATCH anon_hops`
        // pendant la resolution).
        let mut hops = anon_hops;
        let mut paused_now = paused;
        let add_res = if let Some(k) = &pending_key {
            let notify = self.pending_notify(k);
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    k.clone(),
                    PendingDownload {
                        infohash: k.clone(),
                        name: magnet.as_ref().and_then(|m| m.display_name.clone()),
                        anon_hops: hops,
                        paused,
                        added_on: now_unix(),
                    },
                );
            let mut notified = std::pin::pin!(notify.notified());
            loop {
                notified.as_mut().enable();
                let Some(cur) = self
                    .inner
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(k)
                    .map(|p| (p.anon_hops, p.paused))
                else {
                    // Entree retiree entre-temps : suppression par
                    // l'utilisateur — abandon silencieux.
                    break Err(CoreError::Cancelled("resolution magnet abandonnee"));
                };
                (hops, paused_now) = cur;
                let engine = match self.engine_for(hops).await {
                    Ok(e) => e,
                    Err(e) => break Err(e),
                };
                // Magnet anonyme `pending` : le moniteur de swarm
                // n'observe que les torrents materialises — or rqbit
                // resout les metadonnees AVANT de creer le torrent.
                // Sans ce join anticipe, un magnet vers un seeder
                // cache restait en METADATA indefiniment : aucun pair
                // e2e ne pouvait jamais arriver (decouverte pas
                // demarree + `get_by_hash` retournait None).
                let mut extra_peers_rx = None;
                if let (Some(stack), Some(ih)) = (
                    self.inner.ipv8.as_ref(),
                    magnet.as_ref().and_then(|m| m.info_hash_v1),
                ) {
                    if hops > 0 {
                        stack.register_pending_swarm(ih, hops as usize);
                        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                        stack.set_pending_peer_sink(ih, tx);
                        extra_peers_rx = Some(std::sync::Arc::new(std::sync::Mutex::new(Some(rx))));
                    } else {
                        // Lane repassee en clair en vol (`PATCH`) :
                        // demonter un eventuel join precedent.
                        stack.clear_pending_swarm(&ih, false);
                    }
                }
                let opts = AddDownloadOptions {
                    paused: paused_now,
                    output_folder: self.effective_output_dir(destination.clone()),
                    trackers: trackers.clone(),
                    initial_peers: initial_peers.clone(),
                    extra_peers_rx,
                    ..Default::default()
                };
                let fut = engine.add_uri_opts(uri, &opts);
                tokio::pin!(fut);
                tokio::select! {
                    res = &mut fut => break res.map_err(CoreError::from),
                    _ = notified.as_mut() => {
                        // `enable` avant la relecture evite de manquer
                        // une notification arrivee entre les deux.
                        notified.set(notify.notified());
                        tracing::info!(
                            infohash = %k,
                            hops,
                            "resolution magnet relancee apres changement de lane"
                        );
                    }
                }
            }
        } else {
            let engine = self.engine_for(hops).await?;
            engine
                .add_uri_opts(
                    uri,
                    &AddDownloadOptions {
                        paused,
                        output_folder: self.effective_output_dir(destination),
                        trackers: trackers.clone(),
                        initial_peers: initial_peers.clone(),
                        ..Default::default()
                    },
                )
                .await
                .map_err(CoreError::from)
        };
        // Demontage du swarm `pending` enregistre dans la boucle :
        // conserve si le torrent est materialise (le moniteur de
        // swarm reprend le relais), retire sinon — echec ou
        // annulation sans laisser de swarm orphelin.
        if let (Some(stack), Some(m)) = (self.inner.ipv8.as_ref(), magnet.as_ref()) {
            if let Some(ih) = m.info_hash_v1 {
                stack.clear_pending_swarm(&ih, add_res.is_ok());
            }
        }
        let removed = pending_key.as_ref().and_then(|k| {
            let r = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(k);
            self.inner
                .pending_notify
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(k);
            r
        });
        if let Some(g) = pending_guard.as_mut() {
            g.disarm();
        }
        let dl = add_res?;
        self.persist(
            &dl,
            uri,
            PersistParams {
                paused: paused_now,
                anon_hops: hops,
                safe_seeding,
                extra_trackers: trackers,
            },
        )?;
        self.notify_if_private(&dl, hops);
        // Un `PATCH` a pu arriver entre la fin de la derniere
        // resolution et le retrait de `pending` : la derniere cible
        // est appliquee via le chemin normal — le download vient
        // d'etre materialise avec `hops`/`paused_now`.
        if let Some(p) = removed {
            let ih_hex = dl.info_hash_hex();
            if p.anon_hops != hops {
                if let Err(e) = self.update_hops(&ih_hex, p.anon_hops).await {
                    tracing::warn!(
                        infohash = %ih_hex,
                        error = %e,
                        "lane demandee pendant la resolution non appliquee"
                    );
                }
            }
            if p.paused != paused_now {
                let r = if p.paused {
                    self.pause(&ih_hex).await
                } else {
                    self.resume(&ih_hex).await
                };
                if let Err(e) = r {
                    tracing::warn!(
                        infohash = %ih_hex,
                        error = %e,
                        "etat pause demande pendant la resolution non applique"
                    );
                }
            }
        }
        Ok(dl)
    }

    /// Trackers du `download_defaults/trackers_file` (relatif a
    /// `state_dir`), ajoutes a chaque nouveau telechargement comme
    /// le post-handle `ADD_DEFAULT_TRACKERS` Python. Synchronise
    /// d'abord le fichier depuis `trackers_file_sync_url` si configure
    /// (`sync_default_trackers_file` : un fetch par heure maximum).
    async fn default_trackers(&self) -> Vec<String> {
        let dd = self.download_defaults();
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
        // `fetch_checked` fait politique IP + epinglage DNS en une
        // etape : `check_uri_policy` + `reqwest::get` laissait une
        // fenetre TOCTOU (re-resolution DNS vers une IP interne) et
        // `resp.bytes()` n'avait aucun plafond de taille.
        match crate::services::fetch_checked(sync_url, &self.inner.config.ip_policy).await {
            Ok(resp) => match crate::services::read_body_limited(resp).await {
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
        let safe = self.download_defaults().safeseeding_enabled;
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
        self.add_torrent_bytes_anon_inner(
            bytes,
            paused,
            anon_hops,
            safe_seeding,
            destination,
            Vec::new(),
        )
        .await
    }

    /// `add_torrent_bytes_anon` avec `initial_peers` rqbit — banc
    /// live (amorcage deterministe du seeder loopback).
    pub async fn add_torrent_bytes_anon_with_peers(
        &self,
        bytes: Vec<u8>,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
        initial_peers: Vec<std::net::SocketAddr>,
    ) -> Result<Download> {
        self.add_torrent_bytes_anon_inner(
            bytes,
            paused,
            anon_hops,
            safe_seeding,
            destination,
            initial_peers,
        )
        .await
    }

    async fn add_torrent_bytes_anon_inner(
        &self,
        bytes: Vec<u8>,
        paused: bool,
        anon_hops: u32,
        safe_seeding: bool,
        destination: Option<std::path::PathBuf>,
        initial_peers: Vec<std::net::SocketAddr>,
    ) -> Result<Download> {
        // Parsing borne en amont pour extraire l'info-hash a persister.
        let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes)?;
        self.check_low_space();
        // `download_exists` Python : idem magnet — pas de doublon
        // cross-lane qui ecraserait la ligne persistee de l'existant.
        let ih_hex = onionbit_crypto::hash::to_hex(&meta.info_hash);
        if let Some(existing) = self.find_download_hex(&ih_hex) {
            return Ok(existing);
        }
        // Idem `add_download_anon` : un magnet du meme infohash en
        // resolution materialiserait un second download moteur a la
        // fin de sa tache (doublon cross-lane) — le `download_exists`
        // Python couvre aussi l'etat METADATA.
        if self.is_pending(&ih_hex) {
            return Err(CoreError::InvalidState(
                "telechargement deja en cours de resolution",
            ));
        }
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
                    initial_peers,
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
        if meta.private {
            self.notify_private(&dl, anon_hops);
        }
        Ok(dl)
    }

    /// `private` decouvert **apres** l'ajout d'un magnet/URI sur une
    /// lane anonyme : le flag n'est pas dans l'URI — il n'est lisible
    /// qu'apres resolution BEP 9 du metainfo. Notifie l'UI
    /// (`private_torrent_detected`) : aucun chemin anonyme n'existe,
    /// le download doit passer en Clair.
    fn notify_if_private(&self, dl: &Download, anon_hops: u32) {
        let Some(meta) = dl
            .torrent_bytes()
            .and_then(|b| onionbit_format::torrent::TorrentMeta::parse(&b).ok())
        else {
            return;
        };
        if meta.private {
            self.notify_private(dl, anon_hops);
        }
    }

    /// Emet [`Notification::PrivateTorrentDetected`] si la lane est
    /// anonyme — le download ne trouvera jamais de pairs (DHT/PEX
    /// interdits par `private=1`, tracker passkey non relayable).
    /// Aucun blocage : le daemon reste permissif, l'UI avertit.
    fn notify_private(&self, dl: &Download, anon_hops: u32) {
        if anon_hops == 0 {
            return;
        }
        self.inner
            .notifier
            .notify(Notification::PrivateTorrentDetected {
                infohash: dl.info_hash_hex(),
                name: dl.name(),
            });
    }

    /// Cree un jumeau public du download : `private` et trackers
    /// retires du metainfo (nouvel info-hash, DHT/PEX reactives), puis
    /// ajoute sur la lane `anon_hops` **sur les memes fichiers** — le
    /// contenu d'un torrent prive devient seedable par l'essaim
    /// anonyme sans re-telechargement ni fuite de l'URL du tracker.
    /// `anon_hops=None` => `download_defaults/number_hops` (>= 1).
    pub async fn clone_public(&self, id_or_hash: &str, anon_hops: Option<u32>) -> Result<Download> {
        let dl = self
            .find_download(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let bytes = dl
            .torrent_bytes()
            .ok_or(CoreError::InvalidState(
                "metainfo indisponible (magnet non resolu)",
            ))?
            .to_vec();
        let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes)?;
        if !meta.private {
            return Err(CoreError::InvalidState(
                "le torrent n'est pas prive — deja partageable",
            ));
        }
        let public = onionbit_format::torrent::to_public(&bytes)?;
        // `name_subfolder` (parite libtorrent) re-ajoute <nom> sous la
        // destination : pour un multi-fichiers on vise le parent de
        // l'output_folder courant pour retomber sur les memes fichiers.
        let output = dl.output_folder();
        let destination = if meta.files.len() > 1 {
            output.parent().map(std::path::Path::to_path_buf)
        } else {
            Some(output)
        };
        let hops = anon_hops.unwrap_or_else(|| self.download_defaults().number_hops.max(1));
        self.add_torrent_bytes_anon(public, false, hops, hops > 0, destination)
            .await
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
    ) -> onionbit_db::Result<DownloadRow> {
        let dd = self.download_defaults();
        Ok(DownloadRow {
            safe_seeding,
            auto_managed: dd.auto_managed,
            queue_position: onionbit_db::downloads::next_queue_position(c)?,
            completed_dir: (!dd.completed_dir.is_empty()).then(|| dd.completed_dir.clone()),
            channel_download: dd.channel_download,
            add_download_to_channel: dd.add_download_to_channel,
            ..Default::default()
        })
    }

    fn persist(&self, dl: &Download, uri: &str, p: PersistParams) -> Result<()> {
        self.inner.db.with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &DownloadRow {
                    infohash: dl.info_hash().to_vec(),
                    name: dl.name(),
                    source_uri: uri.to_string(),
                    // `add_uri_opts` ne retourne qu'apres resolution
                    // du metainfo — le persister evite de re-resoudre
                    // le magnet a chaque demarrage (meme source que
                    // le checkpoint Python : le `.torrent` sauvegarde).
                    torrent_data: dl.torrent_bytes().map(|b| b.to_vec()),
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
        meta: &onionbit_format::torrent::TorrentMeta,
        p: PersistParams,
    ) -> Result<()> {
        self.inner.db.with(|c| {
            onionbit_db::downloads::upsert(
                c,
                &DownloadRow {
                    infohash: meta.info_hash.to_vec(),
                    name: Some(meta.name.clone()),
                    source_uri: format!(
                        "magnet:?xt=urn:btih:{}",
                        onionbit_crypto::hash::to_hex(&meta.info_hash)
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
            if let Ok(Some(_)) = onionbit_db::channel::get_by_infohash(c, infohash) {
                return Ok(());
            }
            let row = onionbit_db::models::ChannelNodeRow {
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
            let _ = onionbit_db::channel::insert(c, &row);
            Ok(())
        });
    }

    /// Canal de re-ciblage d'une resolution `pending` (get-or-create).
    fn pending_notify(&self, key: &str) -> std::sync::Arc<tokio::sync::Notify> {
        self.inner
            .pending_notify
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// `true` si l'infohash hex designe un magnet en cours de
    /// resolution (`pending`, statut METADATA) — pas encore d'objet
    /// moteur.
    pub fn is_pending(&self, infohash_hex: &str) -> bool {
        onionbit_crypto::hash::from_hex(infohash_hex).is_some_and(|ih| {
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&onionbit_crypto::hash::to_hex(&ih))
        })
    }

    /// `update_hops` Python (`DownloadManager.update_hops`) : retire le
    /// telechargement de son moteur actuel puis le recree sur le moteur
    /// a `new_hops` sauts — les donnees sur disque ET les reglages
    /// persistes sont conserves (comme le `checkpoint` Python survive
    /// a la recreation).
    ///
    /// Un magnet encore en resolution (`pending`, statut METADATA)
    /// n'a pas d'objet moteur : la cible est mise a jour dans
    /// `pending` et la tache de resolution relance `add_uri_opts` sur
    /// la nouvelle lane — comme Python, ou le Download METADATA
    /// accepte `update_hops` (remove + re-add).
    pub async fn update_hops(&self, id_or_hash: &str, new_hops: u32) -> Result<()> {
        let Some(dl) = self.find_download(id_or_hash) else {
            return self.update_pending_hops(id_or_hash, new_hops);
        };
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
        let ih_hex = onionbit_crypto::hash::to_hex(&ih);
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
                    .with(|c| onionbit_db::downloads::upsert(c, &row))?;
                Ok(())
            }
            Err(e) => {
                // Rollback best-effort sur l'ancien moteur : Python
                // laisse le download perdu, on prefere restaurer
                // l'etat initial (ligne DB intacte).
                row.anon_hops = i64::from(old_hops);
                if let Err(rb) = self.readd_row(&old_engine, &row).await {
                    tracing::warn!(
                        infohash = %onionbit_crypto::hash::to_hex(&ih),
                        error = %rb,
                        "update_hops: rollback impossible — download perdu"
                    );
                }
                Err(e)
            }
        }
    }

    /// `update_hops` pour un magnet en resolution (`pending`) : la
    /// cible est relue par la tache de resolution au reveil
    /// (`pending_notify`), puis appliquee a la materialisation par le
    /// chemin normal. La ligne `downloads` persistee suit aussi quand
    /// elle existe (restauration differee) — un ajout magnet frais
    /// n'a pas encore de ligne.
    fn update_pending_hops(&self, id_or_hash: &str, new_hops: u32) -> Result<()> {
        let ih = onionbit_crypto::hash::from_hex(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let key = onionbit_crypto::hash::to_hex(&ih);
        {
            let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            let Some(p) = pending.get_mut(&key) else {
                return Err(CoreError::InvalidState("telechargement inconnu"));
            };
            // Bornes identiques a `engine_for` : une lane invalide ne
            // doit pas rester stockee comme cible de resolution.
            if new_hops > crate::ipv8_stack::MAX_ANON_HOPS as u32 {
                return Err(CoreError::State("anon_hops doit etre entre 1 et 3".into()));
            }
            if new_hops > 0 && self.inner.ipv8.is_none() {
                return Err(CoreError::InvalidState(
                    "anon_hops > 0 mais la stack ipv8 est inactive",
                ));
            }
            if p.anon_hops == new_hops {
                return Ok(());
            }
            p.anon_hops = new_hops;
        }
        if let Some(n) = self
            .inner
            .pending_notify
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            n.notify_one();
        }
        // La ligne existe deja pour les restaurations differees —
        // best-effort : absente pour un ajout frais (persistee a la
        // materialisation).
        let _ = self.update_download_row(&ih, |r| r.anon_hops = i64::from(new_hops));
        Ok(())
    }

    /// Liste les telechargements (moteur principal + lanes anonymes).
    pub fn downloads(&self) -> Vec<DownloadStats> {
        self.all_engines().iter().flat_map(|e| e.list()).collect()
    }

    /// Stats par pair d'un telechargement (tous moteurs — principal
    /// et lanes anonymes ; `get_peers` de `GET /api/downloads`).
    pub fn peer_stats(&self, id_or_hash: &str) -> Option<Vec<onionbit_bittorrent::DownloadPeer>> {
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

    /// Magnets en cours de resolution BEP 9 (`METADATA` Python) —
    /// fusionnes dans `GET /api/downloads` par la couche API.
    pub fn pending_downloads(&self) -> Vec<PendingDownload> {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Lane detenant reellement le telechargement (verite moteur,
    /// contra `downloads.anon_hops` qui est l'intention persistee) :
    /// `0` = moteur principal en clair, `1..=3` = lane anonyme. Sert
    /// de repli au badge `hops` quand la ligne `downloads` est
    /// absente — afficher « Clair » pour un download qui tourne
    /// reellement en tunnel serait un mensonge sur l'anonymat.
    pub fn owner_engine_hops(&self, infohash_hex: &str) -> Option<u32> {
        let ih = onionbit_crypto::hash::from_hex(infohash_hex)?;
        if self.inner.engine.get_by_hash(&ih).is_some() {
            return Some(0);
        }
        self.inner.ipv8.as_ref().and_then(|s| {
            s.anon_engines_with_hops()
                .into_iter()
                .find(|(_, e)| e.get_by_hash(&ih).is_some())
                .map(|(h, _)| h as u32)
        })
    }

    /// `anon_hops` par info-hash hex, d'apres la persistance DB
    /// (utilise par `GET /api/downloads` pour `hops`/`anon_download`).
    pub fn anon_hops_map(&self) -> std::collections::HashMap<String, u32> {
        self.inner
            .db
            .with(onionbit_db::downloads::list)
            .map(|rows| {
                rows.into_iter()
                    .map(|r| {
                        (
                            onionbit_crypto::hash::to_hex(&r.infohash),
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
    ///
    /// Un magnet en resolution (`pending`, etat METADATA) est
    /// pausable comme le Download Python : l'intention est portee
    /// par `pending.paused`, appliquee a la materialisation.
    pub async fn pause(&self, id_or_hash: &str) -> Result<()> {
        let Some(engine) = self.owner_engine(id_or_hash) else {
            return self.set_pending_paused(id_or_hash, true);
        };
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
        let Some(engine) = self.owner_engine(id_or_hash) else {
            return self.set_pending_paused(id_or_hash, false);
        };
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

    /// Pause/reprise sur un magnet en resolution : pas d'objet moteur
    /// — `pending.paused` est relu par la tache de resolution
    /// (`AddDownloadOptions.paused` a chaque relance) puis applique a
    /// la materialisation. La ligne persistee suit quand elle existe
    /// (restauration differee).
    fn set_pending_paused(&self, id_or_hash: &str, paused: bool) -> Result<()> {
        let ih = onionbit_crypto::hash::from_hex(id_or_hash)
            .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
        let key = onionbit_crypto::hash::to_hex(&ih);
        {
            let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            let Some(p) = pending.get_mut(&key) else {
                return Err(CoreError::InvalidState("telechargement inconnu"));
            };
            p.paused = paused;
        }
        let _ = self.update_download_row(&ih, |r| {
            r.paused = paused;
            r.user_stopped = paused;
        });
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
    ///
    /// Un telechargement sans objet moteur (magnet en resolution
    /// `pending`, ligne pas encore reinjectee a la restauration) est
    /// quand meme supprimable : un download Python en etat METADATA
    /// l'est. La suppression porte alors sur l'entree `pending` et
    /// la ligne `downloads`.
    pub async fn remove(&self, id_or_hash: &str, delete_files: bool) -> Result<()> {
        let Some(dl) = self.find_download(id_or_hash) else {
            let ih = onionbit_crypto::hash::from_hex(id_or_hash)
                .ok_or(CoreError::InvalidState("telechargement inconnu"))?;
            let key = onionbit_crypto::hash::to_hex(&ih);
            let pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key)
                .is_some();
            if pending {
                // Reveille la tache de resolution : l'entree retiree
                // lui fait abandonner `add_uri_opts`/`readd_row` en
                // vol au lieu de materialiser puis persister un
                // download supprime.
                if let Some(n) = self
                    .inner
                    .pending_notify
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key)
                {
                    n.notify_one();
                }
            }
            let known = self.row_of(&ih)?.is_some();
            if !pending && !known {
                return Err(CoreError::InvalidState("telechargement inconnu"));
            }
            self.inner
                .db
                .with(|c| onionbit_db::downloads::delete(c, &ih))?;
            return Ok(());
        };
        let infohash = dl.info_hash_hex();
        // Un meme infohash a pu etre materialise sur plusieurs
        // moteurs (lanes anonymes = sessions librqbit distinctes)
        // par une course de resolution : ne retirer que le premier
        // detenteur laissait l'orphelin liste par `downloads()`
        // sans ligne persistee — reaffiche en lane par defaut.
        let mut first_err = None;
        for e in self.all_engines() {
            if e.get(id_or_hash).is_none() {
                continue;
            }
            if let Err(err) = e.remove(id_or_hash, delete_files).await {
                first_err.get_or_insert_with(|| CoreError::from(err));
            }
        }
        if let Some(err) = first_err {
            return Err(err);
        }
        // Une entree `pending` residuelle coexistant avec l'objet
        // moteur : la retirer aussi, sa tache de resolution doit
        // abandonner au lieu de materialiser un download supprime.
        {
            let key = infohash.clone();
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            if let Some(n) = self
                .inner
                .pending_notify
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key)
            {
                n.notify_one();
            }
        }
        {
            let h = infohash;
            // Source importee via le dossier surveille : la retirer
            // aussi, sinon le prochain scan re-importerait le download.
            let watch_dir = self
                .inner
                .services
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .watch_folder
                .as_ref()
                .map(|w| w.directory().to_path_buf());
            if let Some(dir) = watch_dir {
                let h2 = h.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::services::watch_folder::remove_source(&dir, &h2);
                })
                .await;
            }
            if let Some(ih) = onionbit_crypto::hash::from_hex(&h) {
                let _ = self
                    .inner
                    .db
                    .with(|c| onionbit_db::downloads::delete(c, &ih));
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
            .with(|c| onionbit_db::downloads::upsert(c, &row))?;
        self.notify_state(&onionbit_crypto::hash::to_hex(&ih));
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
        // Ne deplacer QUE les fichiers declares par le torrent :
        // `output_folder` est le dossier de telechargements PARTAGE
        // pour un mono-fichier (rqbit n'y ajoute pas de sous-dossier)
        // — parcourir tout le dossier emporterait les autres
        // telechargements et les fichiers personnels de l'utilisateur.
        // Lu avant `remove_engine_only`, tant que les metadonnees
        // sont chargees (magnet non resolu → aucun fichier a bouger).
        let rel_paths: Vec<PathBuf> = dl
            .files()
            .unwrap_or_default()
            .iter()
            .map(|f| PathBuf::from(&f.name))
            .collect();
        // Les handles fichiers doivent etre fermes avant le
        // deplacement (rename impossible sinon sous Windows).
        self.remove_engine_only(id_or_hash, false).await?;
        // `std::fs` bloquant sur de gros volumes : le deplacement est
        // deporte sur le pool de threads bloquants pour ne pas figer
        // l'executor (API, tunnels) pendant la copie inter-volumes.
        let t0 = std::time::Instant::now();
        let src = current.clone();
        let dst = dest_dir.to_path_buf();
        let moved = tokio::task::spawn_blocking({
            let rel_paths = rel_paths.clone();
            move || move_torrent_files(&src, &dst, &rel_paths)
        })
        .await
        .unwrap_or_else(|e| {
            Err(std::io::Error::other(format!(
                "deplacement interrompu: {e}"
            )))
        });
        let elapsed = t0.elapsed();
        if elapsed > std::time::Duration::from_millis(500) {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis() as u64,
                "move_storage : deplacement de fichiers lent"
            );
        }
        if let Err(e) = moved {
            // Rollback best-effort : remettre les fichiers du torrent
            // deja deplaces puis re-add a l'ancien emplacement.
            let src = dest_dir.to_path_buf();
            let dst = current.clone();
            let _ = tokio::task::spawn_blocking(move || move_torrent_files(&src, &dst, &rel_paths))
                .await;
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
            .with(|c| onionbit_db::downloads::upsert(c, &row))?;
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
            let mut rows = onionbit_db::downloads::list(c)?;
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
                onionbit_db::downloads::upsert(c, &r)?;
            }
            Ok(())
        })?)
    }

    /// Marque l'intention pause/reprise dans la persistance
    /// (`user_stopped`/`paused` Python) — appele par le PATCH sur
    /// `state=stop|resume`.
    ///
    /// Magnet en resolution (`pending`) : `pause`/`resume` ont deja
    /// porte l'intention dans `pending.paused` — et dans la ligne
    /// quand elle existe. Sans ligne ni moteur : no-op.
    pub fn set_stopped_flag(&self, id_or_hash: &str, stopped: bool) -> Result<()> {
        let (dl, _) = match self.download_and_row(id_or_hash) {
            Ok(t) => t,
            Err(e) => {
                return if self.is_pending(id_or_hash) {
                    Ok(())
                } else {
                    Err(e)
                };
            }
        };
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
        // Estimateur de capacite upload (`tunnel_community/bandwidth`) :
        // mesure UPnP/sonde periodique + pic passif endpoint ; en mode
        // `max_relayed_rate = -1` applique le plafond AIMD a chaud
        // sur le limiteur tunnel. Sans IPv8 l'endpoint n'existe pas.
        if config.ipv8.enabled {
            let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
            let this = self.clone();
            tokio::spawn(async move {
                crate::services::bandwidth::run_bandwidth_task(this, &mut stop_rx).await;
            });
            self.inner.asyncio.tasks.register(
                Some("CongestionController"),
                "measure",
                Some(config.ipv8.bandwidth.sample_secs as f64),
            );
            services.bandwidth_stop = Some(stop_tx);
        }
        *self
            .inner
            .services
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = services;
    }

    /// Acces au torrent checker (API, tests).
    pub fn torrent_checker(&self) -> Option<Arc<crate::services::torrent_checker::TorrentChecker>> {
        self.inner
            .services
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .checker
            .clone()
    }

    /// Acces au gestionnaire RSS.
    pub fn rss(&self) -> Option<Arc<crate::services::rss::RssManager>> {
        self.inner
            .services
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rss
            .clone()
    }

    /// Stack IPv8 de la session (`None` si `config.ipv8.enabled =
    /// false` — equivalent de `session.ipv8` conditionnel Python).
    pub fn ipv8(&self) -> Option<Arc<crate::ipv8_stack::Ipv8Stack>> {
        self.inner.ipv8.clone()
    }

    /// Contrôleur de congestion du débit servi
    /// (`tunnel_community/bandwidth`) — état RTT + plafond appliqué
    /// (diagnostics).
    pub fn bandwidth(&self) -> Arc<crate::services::bandwidth::CongestionController> {
        self.inner.bandwidth.clone()
    }

    /// Moniteur `/api/ipv8/asyncio/*` (drift, taches, debug).
    pub fn asyncio(&self) -> &crate::asyncio::AsyncioMonitor {
        &self.inner.asyncio
    }

    /// `DHTDiscoveryCommunity` (`None` si IPv8 ou `dht_discovery` est
    /// desactive — `session.get_overlay(DHTCommunity)` Python rend
    /// alors `None` et le `dht_endpoint` repond 404).
    pub fn dht(&self) -> Option<Arc<onionbit_ipv8::dht::DhtCommunity>> {
        self.inner.ipv8.as_ref().and_then(|s| s.dht.clone())
    }

    /// Acces a la base de metadonnees (endpoints `/api/metadata`).
    /// `db_endpoint.augmenter` Python : acces a l'augmenteur de
    /// recherche (`local_search`).
    pub fn augmenter(&self) -> &Arc<crate::augmenter::Augmenter> {
        &self.inner.augmenter
    }

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
        let ov = self
            .inner
            .overrides
            .read()
            .unwrap_or_else(|e| e.into_inner());
        // Sections restart-only : la valeur postee l'emporte sur
        // l'etat de demarrage (effet reel au prochain lancement).
        if let Some(ipv8) = &ov.ipv8 {
            cfg.ipv8 = ipv8.clone();
        }
        if let Some(engine) = &ov.engine {
            cfg.engine = engine.clone();
        }
        if let Some(enabled) = ov.enable_torrent_checker {
            cfg.enable_torrent_checker = enabled;
        }
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
        if let Some(dd) = &ov.download_defaults {
            cfg.download_defaults = dd.clone();
        }
        // Bornes de circuits a chaud : relues dans la stack IPv8.
        if let Some(stack) = self.ipv8() {
            let (min, max) = stack.circuit_bounds();
            cfg.ipv8.min_circuits = min as u32;
            cfg.ipv8.max_circuits = max as u32;
        }
        cfg
    }

    /// `download_defaults` effectif — `POST /api/settings` applique a
    /// chaud (comme le `config` mute in-place de Python).
    fn download_defaults(&self) -> crate::config::DownloadDefaults {
        self.inner
            .overrides
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .download_defaults
            .clone()
            .unwrap_or_else(|| self.inner.config.download_defaults.clone())
    }

    /// Reconfigure les services a chaud (`POST /api/settings`) :
    /// URLs RSS, watch folder et dossier par defaut des telechargements.
    pub fn apply_service_settings(&self, config: &CoreConfig) {
        // Memorise le sous-ensemble applique pour que `effective_config()`
        // (et `GET /api/settings`) reflete le reglage courant.
        *self
            .inner
            .overrides
            .write()
            .unwrap_or_else(|e| e.into_inner()) = ServiceOverrides {
            rss_urls: Some(config.rss_urls.clone()),
            watch_folder_dir: Some(config.watch_folder_dir.clone()),
            download_dir: Some(config.engine.output_dir.clone()),
            queue: Some(config.queue.clone()),
            rate_limits: Some((config.engine.max_upload_bps, config.engine.max_download_bps)),
            download_defaults: Some(config.download_defaults.clone()),
            ipv8: Some(config.ipv8.clone()),
            engine: Some(config.engine.clone()),
            enable_torrent_checker: Some(config.enable_torrent_checker),
        };
        // `min_circuits`/`max_circuits` : Python les lit dans
        // `self.settings` a chaque tick de `monitor_downloads` — le
        // watchdog des lanes relit les bornes partagees a chaud.
        if let Some(stack) = self.ipv8() {
            stack.set_circuit_bounds(
                config.ipv8.min_circuits as usize,
                config.ipv8.max_circuits as usize,
            );
            // `tunnel_community/guards_enabled` (ADR-0010) : bascule a
            // chaud — le store DB est injecte au premier armement et
            // charge alors le set persistant de la table `guards`.
            if let Some(tunnel) = stack.tunnel.as_ref() {
                if config.ipv8.guards_enabled && !tunnel.guards.is_enabled() {
                    tunnel.set_guard_store(Arc::new(crate::guard_store::DbGuardStore::new(
                        self.inner.db.clone(),
                    )));
                }
                tunnel.guards.set_enabled(config.ipv8.guards_enabled);
                // `tunnel_community/ledger_*` (ADR-0015) : bascule a
                // chaud de la collecte et de la gate — les compteurs
                // sont conserves, simplement plus alimentes quand la
                // collecte retombe (la gate exige les deux). Le store
                // DB est injecte au premier armement comme les
                // guards.
                if config.ipv8.ledger_enabled && !tunnel.ledger.is_enabled() {
                    tunnel.set_peer_stats_store(Arc::new(
                        crate::peer_stats_store::DbPeerStatsStore::new(self.inner.db.clone()),
                    ));
                }
                tunnel.ledger.set_enabled(config.ipv8.ledger_enabled);
                tunnel.ledger.set_enforce(config.ipv8.ledger_enforce);
                // `tunnel_community/max_relayed_rate` (extension Rust)
                // : seau a jetons de la pompe d'emission, reglage a
                // chaud — `-1` = auto (upload mesure × share, via
                // l'estimateur), `0` = illimite, `>0` = fixe.
                let rate = if config.ipv8.max_relayed_bps < 0 {
                    self.inner.bandwidth.current_bps(&config.ipv8.bandwidth)
                } else {
                    config.ipv8.max_relayed_bps as u64
                };
                tunnel.set_relay_rate_bps(rate);
            }
        }
        // `set_session_limits` Python : les bornes de debit de
        // session s'appliquent a chaud sur toutes les lanes
        // (`Session::ratelimits` rqbit est mutable). La file
        // (`active_*`) est relue par `enforce_queue_limits` au tick
        // suivant via `effective_config`.
        for engine in self.all_engines() {
            engine.set_ratelimits(config.engine.max_upload_bps, config.engine.max_download_bps);
        }
        let mut services = self
            .inner
            .services
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        // Watch folder : redemarrage si le repertoire OU l'intervalle
        // change (le tick est fige a la creation du service).
        let interval = std::time::Duration::from_millis(config.watch_folder_interval_ms);
        let current = services
            .watch_folder
            .as_ref()
            .map(|w| (w.directory().to_path_buf(), w.interval()));
        if current != config.watch_folder_dir.clone().map(|d| (d, interval)) {
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
        let services = std::mem::take(
            &mut *self
                .inner
                .services
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
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
        if let Some(tx) = &services.bandwidth_stop {
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
        // `on_shutdown` de `AugmentedSearch` : persiste la fenetre de
        // titres en attente pour le prochain demarrage.
        self.inner.augmenter.flush_cache();
        // Checkpoint WAL final : le journal est rejoue dans le fichier
        // principal puis tronque — le prochain demarrage part d'une
        // base compacte au lieu de rejouer plusieurs Mio de WAL.
        let db = self.inner.db.clone();
        let _ = tokio::task::spawn_blocking(move || db.checkpoint()).await;
        self.shutdown_state("Shutting down GUI connection. Going dark.");
    }
}

/// Deplace les fichiers declares d'un torrent de `src` vers `dst`
/// (`move_storage` : `rename` intra-volume ; copie + suppression en
/// secours pour les deplacements inter-volumes). `rel_paths` =
/// `relative_filename` de chaque fichier du torrent — SEULS ces
/// fichiers bougent : l'`output_folder` source peut etre le dossier
/// de telechargements partage, qui n'est jamais vide lui-meme.
fn move_torrent_files(src: &Path, dst: &Path, rel_paths: &[PathBuf]) -> std::io::Result<()> {
    for rel in rel_paths {
        let from = src.join(rel);
        let to = dst.join(rel);
        if !from.exists() || to.exists() {
            // Fichier absent (telechargement partiel, padding) : rien
            // a deplacer. Collision : la destination prevaut — les
            // pieces non deplacees seront reverifiees au re-add.
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
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
    // Sous-dossiers devenus vides dans `src` (best-effort — le
    // dossier racine `src` est conserve expres : il peut etre le
    // dossier de telechargements partage de l'utilisateur).
    // `Path::parent` d'un chemin sans dossier vaut `Some("")` et
    // `src.join("")` == `src` : le parent vide est filtre expres pour
    // que la racine ne figure jamais dans `dirs`.
    let mut dirs: Vec<PathBuf> = rel_paths
        .iter()
        .filter_map(|rel| rel.parent().filter(|p| !p.as_os_str().is_empty()))
        .map(|p| src.join(p))
        .filter(|d| d != src)
        .collect();
    dirs.sort_unstable_by_key(|p| std::cmp::Reverse(p.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(&d); // echoue si non vide — voulu
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
