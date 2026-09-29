//! Lignes du domaine persiste (miroir des entites Pony de
//! `tribler.core.database.orm_bindings`, types Rust idiomatiques).

/// `torrent_state` : sante d'un essaim (metriques du TorrentChecker).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TorrentStateRow {
    /// rowid (0 = non insere).
    pub rowid: i64,
    /// Info-hash v1 (20 octets).
    pub infohash: Vec<u8>,
    /// Seeders observes.
    pub seeders: i64,
    /// Leechers observes.
    pub leechers: i64,
    /// Dernier controle (secondes Unix).
    pub last_check: i64,
    /// Controle fait par nous (pas juste relaye).
    pub self_checked: bool,
    /// Le contenu est present localement.
    pub has_data: bool,
    /// Tracker preferentiel associe (rowid de `tracker_state`).
    pub tracker_id: Option<i64>,
}

/// `tracker_state` : tracker connu.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackerStateRow {
    /// rowid (0 = non insere).
    pub rowid: i64,
    /// URL normalisee du tracker.
    pub url: String,
    /// Dernier controle (secondes Unix).
    pub last_check: i64,
    /// Tracker joignable.
    pub alive: bool,
    /// Echecs consecutifs.
    pub failures: i64,
}

/// `channel_node` : entree de metadonnees (torrent regular, canal,
/// collection…). `metadata_type` suit `tribler_format::mdblob::types`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelNodeRow {
    /// rowid (0 = non insere).
    pub rowid: i64,
    /// Info-hash du contenu (pour un canal : cle publique du canal).
    pub infohash: Vec<u8>,
    /// Taille du contenu.
    pub size: i64,
    /// Date du torrent (secondes Unix).
    pub torrent_date: i64,
    /// `tracker_info` (champ deprecated, conserve pour fidelite).
    pub tracker_info: String,
    /// Titre.
    pub title: String,
    /// Tags.
    pub tags: String,
    /// Type de metadonnee (`mdblob::types::*`).
    pub metadata_type: i64,
    /// Flags reserves.
    pub reserved_flags: i64,
    /// Identifiant du parent (0 = racine).
    pub origin_id: i64,
    /// Cle publique du signataire (64 octets).
    pub public_key: Vec<u8>,
    /// Identifiant unique dans le canal (`id_`).
    pub id_: i64,
    /// Timestamp de creation du payload (ms Unix cote Python).
    pub timestamp: i64,
    /// Signature Ed25519 (`None` = entree free-for-all).
    pub signature: Option<Vec<u8>>,
    /// Date d'insertion locale (secondes Unix).
    pub added_on: i64,
    /// Statut (1 = COMMITTED ; valeurs Python `MetadataStatus`).
    pub status: i64,
    /// Score de contenu adulte.
    pub xxx: f64,
    /// rowid de `torrent_state` associe (sante).
    pub health_rowid: Option<i64>,
    /// Version du processeur de tags appliquee.
    pub tag_processor_version: i64,
    /// `health.seeders` Python : sante jointe de `torrent_state`
    /// (peuplee uniquement par les requetes avec `LEFT JOIN`,
    /// jamais persistee par `insert`).
    pub health_seeders: Option<i64>,
    /// `health.leechers` Python.
    pub health_leechers: Option<i64>,
    /// `health.last_check` Python (`last_tracker_check` du JSON).
    pub health_last_check: Option<i64>,
}

/// `downloads` : telechargement connu du daemon (persistant entre
/// redemarrages — la reprise des pieces est assuree par librqbit).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DownloadRow {
    /// rowid (0 = non insere).
    pub rowid: i64,
    /// Info-hash v1 (20 octets).
    pub infohash: Vec<u8>,
    /// Nom du contenu (si connu).
    pub name: Option<String>,
    /// URI d'origine (magnet ou http).
    pub source_uri: String,
    /// Octets du fichier `.torrent` (si disponibles).
    pub torrent_data: Option<Vec<u8>>,
    /// Repertoire de sortie.
    pub output_dir: String,
    /// Date d'ajout (secondes Unix).
    pub added_on: i64,
    /// Ajoute en pause.
    pub paused: bool,
    /// Telechargement termine.
    pub finished: bool,
    /// Nombre de sauts anonymes du tunnel (0 = telechargement direct).
    pub anon_hops: i64,
    /// Seeding anonyme (`safe_seeding` du DownloadConfig Python).
    pub safe_seeding: bool,
    /// Arret demande par l'utilisateur (`user_stopped` Python) —
    /// distinct de `paused` (etat courant) comme en Python.
    pub user_stopped: bool,
    /// Limite d'upload en octets/s (0 = illimite ; Python stocke -1).
    pub upload_limit: i64,
    /// Limite de download en octets/s (0 = illimite).
    pub download_limit: i64,
    /// Ratio de seed individuel (`None` = defaut global
    /// `download_defaults/seeding_ratio`, comme `config.get_seeding_ratio`).
    pub seeding_ratio: Option<f64>,
    /// File d'attente auto-managee (persiste pour fidelite ; librqbit
    /// n'a pas de queue — attribut logique).
    pub auto_managed: bool,
    /// Position dans la file (-1 = non positionne).
    pub queue_position: i64,
    /// Dossier des fichiers termines (`completed_dir` Python).
    pub completed_dir: Option<String>,
    /// Fichiers selectionnes (indices ; `None` = tous, CSV en base
    /// comme `download_defaults/files` Python).
    pub selected_files: Option<Vec<i64>>,
    /// Priorites par fichier (indices -> priorite 0..7 ; persiste
    /// pour reporting — librqbit n'ordonnance pas par priorite).
    pub file_priorities: Option<Vec<i64>>,
    /// Trackers ajoutes a chaud (`PUT .../trackers` ; rejoues au
    /// re-add, une URL par ligne en base).
    pub extra_trackers: Vec<String>,
    /// Trackers retires a chaud (`DELETE .../trackers` ; filtres des
    /// trackers de la source au re-add — librqbit ne permet pas le
    /// retrait sur un torrent actif, une URL par ligne en base).
    pub removed_trackers: Vec<String>,
    /// Timestamp de completion (`atp.completed_time` Python ; 0 si
    /// pas termine).
    pub time_finished: i64,
    /// Marqueur de telechargement de canal (`channel_download` du
    /// `DownloadConfig` Python — persiste pour la future passe canaux ;
    /// les canaux ne sont pas encore portes).
    pub channel_download: bool,
    /// Ajouter le telechargement termine au canal de l'utilisateur
    /// (`add_download_to_channel` Python — meme reserve).
    pub add_download_to_channel: bool,
}
