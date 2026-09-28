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
        self.session.stop().await;
        tracing::info!("session bittorrent arretee");
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
    pub fn get(&self, id_or_hash: &str) -> Option<Download> {
        let key = parse_id_or_hash(id_or_hash)?;
        self.session.get(key).map(|inner| self.wrap(inner))
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
}

/// Parse `id_or_hash` : id numerique (`usize`) ou info-hash hex 40c.
fn parse_id_or_hash(s: &str) -> Option<TorrentIdOrHash> {
    if let Ok(id) = s.parse::<usize>() {
        return Some(TorrentIdOrHash::Id(id));
    }
    if s.len() == 40 {
        let bytes = hex::decode(s).ok()?;
        let mut id = [0u8; 20];
        id.copy_from_slice(&bytes);
        return Some(TorrentIdOrHash::Hash(librqbit_core::hash_id::Id20::new(id)));
    }
    None
}
