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
}
