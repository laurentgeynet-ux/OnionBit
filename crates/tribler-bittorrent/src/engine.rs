//! Moteur BitTorrent : enveloppe de `librqbit::Session`.
//!
//! `BtEngine` est le port "infrastructure" du domaine : il gere le
//! cycle de vie de la session (creation, arret) et l'ajout/suppression/
//! pause/reprise des telechargements. Il ne connait ni REST ni base de
//! donnees : ces couches vivent dans `tribler-api` et `tribler-db`.

use std::sync::Arc;

use librqbit::api::TorrentIdOrHash;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse};

use crate::config::EngineConfig;
use crate::download::{Download, DownloadStats};
use crate::error::{BtError, Result};

/// Moteur BitTorrent du daemon (equivalent de
/// `tribler.core.libtorrent.download_manager.DownloadManager`).
///
/// Clone leger (`Arc` interne).
#[derive(Clone)]
pub struct BtEngine {
    session: Arc<librqbit::Session>,
    config: EngineConfig,
}

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
        let session = librqbit::Session::new_with_opts(
            config.output_dir.clone(),
            config.to_session_options(),
        )
        .await
        .map_err(|e| BtError::Engine(e.to_string()))?;
        tracing::info!(
            output_dir = %config.output_dir.display(),
            dht = config.enable_dht,
            listen = ?config.listen_port,
            "session bittorrent demarree"
        );
        Ok(Self { session, config })
    }

    /// Arret propre de la session et de toutes ses taches.
    pub async fn stop(&self) {
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
        let response = self
            .session
            .add_torrent(add, opts)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))?;
        match response {
            AddTorrentResponse::AlreadyManaged(_, handle)
            | AddTorrentResponse::Added(_, handle) => Ok(Download { inner: handle }),
            AddTorrentResponse::ListOnly(_) => Err(BtError::NoHandle),
        }
    }

    /// Liste les telechargements courants (instantanes d'etat).
    pub fn list(&self) -> Vec<DownloadStats> {
        self.session.with_torrents(|it| {
            it.map(|(_, t)| Download {
                inner: Arc::clone(t),
            })
            .map(|d| d.stats())
            .collect()
        })
    }

    /// Recupere un telechargement par id interne ou info-hash hex.
    pub fn get(&self, id_or_hash: &str) -> Option<Download> {
        let key = parse_id_or_hash(id_or_hash)?;
        self.session.get(key).map(|inner| Download { inner })
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
        let d = self
            .get(id_or_hash)
            .ok_or_else(|| BtError::NotFound(id_or_hash.to_string()))?;
        self.session
            .unpause(&d.inner)
            .await
            .map_err(|e| BtError::Engine(e.to_string()))
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
