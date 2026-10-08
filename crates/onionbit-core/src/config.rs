// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration de la session coeur.
//!
//! Regroupe tous les reglages d'orchestration (les reglages du moteur
//! BitTorrent vivent dans `onionbit_bittorrent::EngineConfig`, ceux du
//! daemon HTTP dans `onionbit-daemon`).

use std::path::PathBuf;

/// Intervalle par defaut d'emission des notifications de progression
/// (le Notifier Python de Tribler emet `torrents_status_update` selon
/// un rythme similaire).
pub const DEFAULT_PROGRESS_INTERVAL_MS: u64 = 1000;

/// Reglages par defaut des nouveaux telechargements
/// (`libtorrent/download_defaults` de `TriblerConfig` Python).
///
/// Ces valeurs initialisent les reglages persistes de chaque
/// telechargement ajoute (equivalent du `DownloadConfig` copie depuis
/// `config.libtorrent.download_defaults` dans `start_download`).
#[derive(Debug, Clone)]
pub struct DownloadDefaults {
    /// Anonymat par defaut a l'ajout (`anonymity_enabled`).
    pub anonymity_enabled: bool,
    /// Nombre de sauts anonymes par defaut (`number_hops`).
    pub number_hops: u32,
    /// Seeding sur par defaut (`safeseeding_enabled`).
    pub safeseeding_enabled: bool,
    /// Mode de seed : `forever` / `never` / `ratio` / `time`
    /// (`seeding_mode` — politique d'arret appliquee par la boucle de
    /// progression).
    pub seeding_mode: String,
    /// Ratio upload/download cible en mode `ratio` (`seeding_ratio`).
    pub seeding_ratio: f64,
    /// Duree de seed en secondes en mode `time` (`seeding_time`).
    pub seeding_time: f64,
    /// File d'attente auto-managee par defaut (`auto_managed`).
    pub auto_managed: bool,
    /// Dossier des fichiers termines (`completed_dir` — vide =
    /// desactive).
    pub completed_dir: String,
    /// Fichier de trackers par defaut (`trackers_file` — relatif a
    /// `state_dir`, vide = aucun).
    pub trackers_file: String,
    /// URL de synchronisation du fichier de trackers
    /// (`trackers_file_sync_url` Python — vide = desactivee).
    pub trackers_file_sync_url: String,
    /// Dossier de sauvegarde des `.torrent` ajoutes
    /// (`download_defaults/torrent_folder` Python —
    /// `write_backup_torrent_file` : `<name> [<infohash>].torrent`
    /// ecrit quand le metainfo est connu ; vide = desactive).
    pub torrent_folder: String,
    /// Marqueur de telechargement de canal (`channel_download`
    /// Python — persiste par telechargement, comportement canal
    /// non porte).
    pub channel_download: bool,
    /// Ajout du telechargement au canal a completion
    /// (`add_download_to_channel` Python — meme reserve).
    pub add_download_to_channel: bool,
}

impl Default for DownloadDefaults {
    /// Memes defauts que `TriblerConfig.libtorrent.download_defaults`.
    fn default() -> Self {
        Self {
            anonymity_enabled: true,
            number_hops: 1,
            safeseeding_enabled: true,
            seeding_mode: "forever".into(),
            seeding_ratio: 2.0,
            seeding_time: 60.0,
            auto_managed: false,
            completed_dir: String::new(),
            trackers_file: String::new(),
            trackers_file_sync_url: String::new(),
            torrent_folder: String::new(),
            channel_download: false,
            add_download_to_channel: false,
        }
    }
}

/// Zone de stockage d'un telechargement (`storage_area`, ADR-0018).
///
/// `public` : contenu en clair sous `data/public/…`. `private` :
/// contenu chiffre `OBD` lie a l'identite sous `data/private/…`
/// (etapes 60-62 — la zone refuse tout ajout tant que
/// `EncryptedStorageFactory` n'est pas cablee).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageArea {
    /// Zone publique en clair (`data/public/`).
    #[default]
    Public,
    /// Zone privee chiffree liee a l'identite (`data/private/`).
    Private,
}

impl StorageArea {
    /// Valeur persistee (`storage/default_area`, colonne
    /// `downloads.storage_area`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }

    /// Parsing permissif : `private` (insensible a la casse) →
    /// `Private`, toute autre valeur → `Public` avec trace (un libelle
    /// inconnu ne doit jamais activer la zone chiffree par accident).
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("private") {
            Self::Private
        } else {
            if !s.eq_ignore_ascii_case("public") {
                tracing::warn!(value = s, "storage/default_area inconnu — repli sur public");
            }
            Self::Public
        }
    }
}

/// Bornes de `storage/private_chunk_kib` (ADR-0018 : 16 Kio = bloc
/// BitTorrent, borne l'amplification des RMW chiffrees ; 4 Kio..=1 Mio).
pub const MIN_PRIVATE_CHUNK_KIB: i64 = 4;
/// Borne haute de `storage/private_chunk_kib`.
pub const MAX_PRIVATE_CHUNK_KIB: i64 = 1024;

/// Reglages `storage/*` (ADR-0018) : zones de telechargement et
/// deplacement a completion.
#[derive(Debug, Clone)]
pub struct StorageSettings {
    /// Zone privee chiffree disponible (`private_enabled`) — etapes
    /// 60-62 : fichiers `OBD`, factory opaque, manifest `OBM`.
    pub private_enabled: bool,
    /// Zone des nouveaux ajouts sans choix explicite (`default_area`).
    pub default_area: StorageArea,
    /// Taille de chunk `OBD` en octets (`private_chunk_kib` borne,
    /// defaut 16 Kio).
    pub private_chunk_bytes: usize,
    /// `temp` → `downloads` a la transition `finished`
    /// (`move_on_completion`, defaut actif sous le layout portable).
    /// `false` = comportement historique : l'ajout ecrit directement
    /// dans le dossier final.
    pub move_on_completion: bool,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            private_enabled: true,
            default_area: StorageArea::Public,
            private_chunk_bytes: (MIN_PRIVATE_CHUNK_KIB.max(16) as usize) * 1024,
            move_on_completion: true,
        }
    }
}

/// Limites de file libtorrent (`libtorrent/active_downloads`,
/// `active_seeds`, `active_limit` Tribler ; `< 0` = illimite).
///
/// librqbit n'a pas de gestionnaire de file : `CoreSession` les
/// applique en pause/reprise des telechargements `auto_managed` (les
/// autres restent hors file, comme les torrents non auto-manages chez
/// libtorrent). Les pauses de file ne modifient pas `paused`/`user_stopped`
/// persistes — distinction `queued` Python.
#[derive(Debug, Clone)]
pub struct QueueLimits {
    /// Telechargements actifs simultanes.
    pub active_downloads: i64,
    /// Seeds actifs simultanes.
    pub active_seeds: i64,
    /// Torrents actifs simultanes au total (download + seed).
    pub active_limit: i64,
}

impl QueueLimits {
    /// Aucune borne effective (toutes les limites `< 0` = illimite).
    pub fn disabled(&self) -> bool {
        self.active_downloads < 0 && self.active_seeds < 0 && self.active_limit < 0
    }
}

impl Default for QueueLimits {
    /// Memes defauts que `TriblerConfig.libtorrent` (3/5/500).
    fn default() -> Self {
        Self {
            active_downloads: 3,
            active_seeds: 5,
            active_limit: 500,
        }
    }
}

/// Configuration de [`crate::CoreSession`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Repertoire d'etat du daemon (base SQLite, resume, cles).
    pub state_dir: PathBuf,
    /// Repertoire de telechargement par defaut.
    pub downloads_dir: PathBuf,
    /// Nom du fichier de base (relatif a `state_dir`).
    pub db_filename: String,
    /// Intervalle de publication des stats de progression.
    pub progress_interval_ms: u64,
    /// Politique anti-SSRF appliquee aux URI distantes (`http(s)`)
    /// ajoutees comme telechargements : resolution DNS puis refus de
    /// toute adresse non autorisee. `strict` en production,
    /// `permissive` en test offline.
    pub ip_policy: onionbit_network_policy::IpPolicy,
    /// Repertoire surveille par le watch folder (`None` = desactive,
    /// `watch_folder/directory` Python).
    pub watch_folder_dir: Option<PathBuf>,
    /// Intervalle de scan du watch folder (ms).
    pub watch_folder_interval_ms: u64,
    /// Flux RSS surveilles (`rss` Python — liste vide = desactive).
    pub rss_urls: Vec<String>,
    /// Active le controle periodique de sante des torrents
    /// (`torrent_checker`).
    pub enable_torrent_checker: bool,
    /// Intervalle de controle du torrent checker (ms).
    pub torrent_checker_interval_ms: u64,
    /// Stack IPv8 de session (decouverte, content discovery, tunnels
    /// anonymes — etape 15). `enabled = false` par defaut.
    pub ipv8: crate::ipv8_stack::Ipv8Config,
    /// Configuration du moteur BitTorrent sous-jacent.
    pub engine: onionbit_bittorrent::EngineConfig,
    /// Reglages par defaut appliques aux nouveaux telechargements
    /// (`download_defaults` Python).
    pub download_defaults: DownloadDefaults,
    /// Limites de file (`libtorrent/active_*` Python).
    pub queue: QueueLimits,
    /// Reverifie les pieces d'un telechargement a la fin
    /// (`libtorrent/check_after_complete` Python — `session.recheck`
    /// cote moteur, rqbit n'ayant pas de recheck in-place).
    pub check_after_complete: bool,
    /// `identity.at_rest` (ADR-0016) : la graine est scellee `OBSK`
    /// sur disque ; le boot entre en phase `Locked` (etape 48d).
    /// Refuse avec `stealth.role != client`.
    pub identity_at_rest: bool,
    /// `identity.seed_acknowledged` : l'utilisateur a confirme avoir
    /// note sa phrase — informatif (UI), ne bloque rien.
    pub identity_seed_acknowledged: bool,
    /// Section `storage` (ADR-0018) : zones public/privee et
    /// deplacement `temp` → `downloads` a la completion (etape 59).
    pub storage: StorageSettings,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".onionbit"),
            downloads_dir: PathBuf::from("downloads"),
            db_filename: "onionbit.db".into(),
            progress_interval_ms: DEFAULT_PROGRESS_INTERVAL_MS,
            ip_policy: onionbit_network_policy::IpPolicy::strict(),
            watch_folder_dir: None,
            watch_folder_interval_ms: crate::services::watch_folder::DEFAULT_CHECK_INTERVAL
                .as_millis() as u64,
            rss_urls: Vec::new(),
            enable_torrent_checker: true,
            torrent_checker_interval_ms: 10_000,
            ipv8: crate::ipv8_stack::Ipv8Config::default(),
            engine: onionbit_bittorrent::EngineConfig::default(),
            download_defaults: DownloadDefaults::default(),
            queue: QueueLimits::default(),
            check_after_complete: false,
            identity_at_rest: false,
            identity_seed_acknowledged: false,
            storage: StorageSettings::default(),
        }
    }
}

impl CoreConfig {
    /// Chemin complet du fichier de base.
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join(&self.db_filename)
    }

    /// Configuration isolee pour les tests : aucun trafic sortant,
    /// repertoire temporaire, base en memoire geree par l'appelant.
    pub fn offline(state_dir: PathBuf) -> Self {
        // ADR-0018 : la zone publique `data/public/downloads` est le
        // defaut — sous un `state_dir` de test `data/` vit dedans
        // (`PathRoots::for_state_dir`).
        let downloads = crate::paths::PathRoots::for_state_dir(&state_dir).public_downloads();
        Self {
            engine: onionbit_bittorrent::EngineConfig::offline(downloads.clone()),
            downloads_dir: downloads,
            state_dir,
            db_filename: "onionbit.db".into(),
            progress_interval_ms: DEFAULT_PROGRESS_INTERVAL_MS,
            ip_policy: onionbit_network_policy::IpPolicy::permissive(),
            watch_folder_dir: None,
            watch_folder_interval_ms: crate::services::watch_folder::DEFAULT_CHECK_INTERVAL
                .as_millis() as u64,
            rss_urls: Vec::new(),
            enable_torrent_checker: false,
            torrent_checker_interval_ms: 10_000,
            ipv8: crate::ipv8_stack::Ipv8Config::default(),
            // Les defauts de telechargement s'appliquent aussi en
            // offline : la politique de seed et les tests de
            // persistance en dependent.
            download_defaults: DownloadDefaults::default(),
            queue: QueueLimits::default(),
            check_after_complete: false,
            identity_at_rest: false,
            identity_seed_acknowledged: false,
            // `move_on_completion = false` en offline : les tests
            // pre-ecrivent le contenu dans `engine.output_dir` et
            // attendent `output_folder` direct — le split
            // `temp → downloads` est couvert par des tests dedies
            // qui activent le drapeau explicitement.
            storage: StorageSettings {
                move_on_completion: false,
                ..StorageSettings::default()
            },
        }
    }
}
