//! Migrations du schema SQLite.
//!
//! Chaque entree de [`MIGRATIONS`] est appliquée dans l'ordre ; le
//! numero de version courant est stocke dans `PRAGMA user_version`.
//!
//! Fidelite a la reference : la structure reprend les entites Pony ORM
//! de `tribler.core.database.orm_bindings` (v15 cote Python), mais les
//! `datetime` sont stockes en **secondes Unix** (`INTEGER`) et les
//! booléens en `INTEGER` 0/1. Le fichier de base n'est **pas**
//! compatible octet-pour-octet avec la DB Python (l'objectif est la
//! semantique du schema, pas l'interoperabilite binaire).

/// Version courante du schema de ce crate.
pub const SCHEMA_VERSION: i64 = 3;

/// Script SQL de chaque migration, dans l'ordre (index 0 = v1).
pub const MIGRATIONS: &[&str] = &[
    "
-- Cle/valeur divers (db_version, reglages persistants).
CREATE TABLE misc (
    name  TEXT PRIMARY KEY,
    value TEXT
);

-- Sante d'un essaim (equivalent de orm_bindings/torrent_state.py).
CREATE TABLE torrent_state (
    rowid        INTEGER PRIMARY KEY AUTOINCREMENT,
    infohash     BLOB NOT NULL UNIQUE,          -- 20 octets SHA-1
    seeders      INTEGER NOT NULL DEFAULT 0,
    leechers     INTEGER NOT NULL DEFAULT 0,
    last_check   INTEGER NOT NULL DEFAULT 0,    -- secondes Unix
    self_checked INTEGER NOT NULL DEFAULT 0,
    has_data     INTEGER NOT NULL DEFAULT 0,
    tracker_id   INTEGER
);

-- Tracker connu (orm_bindings/tracker_state.py).
CREATE TABLE tracker_state (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    url         TEXT NOT NULL UNIQUE,
    last_check  INTEGER NOT NULL DEFAULT 0,
    alive       INTEGER NOT NULL DEFAULT 1,
    failures    INTEGER NOT NULL DEFAULT 0
);

-- Relation N-N essaim <-> trackers (torrent_state.trackers).
CREATE TABLE torrent_state_tracker (
    torrent_state_rowid INTEGER NOT NULL REFERENCES torrent_state(rowid),
    tracker_state_rowid INTEGER NOT NULL REFERENCES tracker_state(rowid),
    PRIMARY KEY (torrent_state_rowid, tracker_state_rowid)
);

-- Noeud de canal / metadonnees de torrent
-- (orm_bindings/torrent_metadata.py, table discriminee \"ChannelNode\").
CREATE TABLE channel_node (
    rowid          INTEGER PRIMARY KEY AUTOINCREMENT,
    infohash       BLOB NOT NULL,
    size           INTEGER NOT NULL DEFAULT 0,
    torrent_date   INTEGER NOT NULL DEFAULT 0,  -- secondes Unix
    tracker_info   TEXT NOT NULL DEFAULT '',    -- deprecated en Python
    title          TEXT NOT NULL DEFAULT '',
    tags           TEXT NOT NULL DEFAULT '',
    metadata_type  INTEGER NOT NULL,            -- cf. tribler_format::mdblob::types
    reserved_flags INTEGER NOT NULL DEFAULT 0,
    origin_id      INTEGER NOT NULL DEFAULT 0,
    public_key     BLOB NOT NULL,
    id_            INTEGER NOT NULL,
    timestamp      INTEGER NOT NULL DEFAULT 0,
    signature      BLOB UNIQUE,                 -- NULL = entree FFA
    added_on       INTEGER NOT NULL DEFAULT 0,
    status         INTEGER NOT NULL DEFAULT 1,  -- COMMITTED
    xxx            REAL NOT NULL DEFAULT 0,
    health_rowid   INTEGER REFERENCES torrent_state(rowid),
    tag_processor_version INTEGER NOT NULL DEFAULT 0,
    UNIQUE (public_key, id_)
);
CREATE INDEX idx_channel_node_infohash ON channel_node(infohash);
CREATE INDEX idx_channel_node_torrent_date ON channel_node(torrent_date);
CREATE INDEX idx_channel_node_origin_id ON channel_node(origin_id);
CREATE INDEX idx_channel_node_pk_origin ON channel_node(public_key, origin_id);

-- Telechargements connus du daemon (pour restaurer la session).
-- Tribler Python delegue la reprise a libtorrent ; ici on persiste la
-- liste des torrents ajoutes (la reprise des pieces est assuree par
-- le fastresume de librqbit).
CREATE TABLE downloads (
    rowid        INTEGER PRIMARY KEY AUTOINCREMENT,
    infohash     BLOB NOT NULL UNIQUE,
    name         TEXT,
    source_uri   TEXT NOT NULL,                 -- magnet ou URI d'origine
    torrent_data BLOB,                          -- octets .torrent si connus
    output_dir   TEXT NOT NULL,
    added_on     INTEGER NOT NULL DEFAULT 0,
    paused       INTEGER NOT NULL DEFAULT 0,
    finished     INTEGER NOT NULL DEFAULT 0
);
",
    // v2 : telechargements anonymes (`anon_hops` cote Tribler) — nombre
    // de sauts du tunnel associe au telechargement (0 = non anonyme).
    "
ALTER TABLE downloads ADD COLUMN anon_hops INTEGER NOT NULL DEFAULT 0;
",
    // v3 : reglages par telechargement (equivalent du `DownloadConfig`
    // checkpointe par Tribler dans `dlcheckpoints/<infohash>.conf`).
    // - `safe_seeding`, `auto_managed`, `user_stopped` : booleens.
    // - `upload_limit`/`download_limit` : octets/s (0 = illimite —
    //   convention Python `-1` ramenee a 0 cote REST).
    // - `seeding_ratio` : override individuel (NULL = defaut global
    //   `libtorrent/download_defaults/seeding_ratio`).
    // - `queue_position` : position persistee (librqbit n'a pas de
    //   file d'attente — attribut logique ordonnant le listing).
    // - `completed_dir` : dossier de fichiers termines (deplacement
    //   post-completion, `move_storage`).
    // - `selected_files` / `file_priorities` : CSV d'entiers comme le
    //   champ `files` du checkpoint Python (`"0,2,3,"`).
    // - `extra_trackers` : URLs ajoutees a chaud (une par ligne) —
    //   rejouees au re-add.
    // - `time_finished` : `atp.completed_time` Python.
    "
ALTER TABLE downloads ADD COLUMN safe_seeding INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN user_stopped INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN upload_limit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN download_limit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN seeding_ratio REAL;
ALTER TABLE downloads ADD COLUMN auto_managed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN queue_position INTEGER NOT NULL DEFAULT -1;
ALTER TABLE downloads ADD COLUMN completed_dir TEXT;
ALTER TABLE downloads ADD COLUMN selected_files TEXT;
ALTER TABLE downloads ADD COLUMN file_priorities TEXT;
ALTER TABLE downloads ADD COLUMN extra_trackers TEXT;
ALTER TABLE downloads ADD COLUMN time_finished INTEGER NOT NULL DEFAULT 0;
",
];

/// Applique les migrations en attente sur une connexion ouverte.
///
/// Erreur `SchemaTooNew` si la base est plus recente que ce crate.
pub fn migrate(conn: &mut rusqlite::Connection) -> crate::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(crate::DbError::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i as i64) + 1;
        if current < target {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", target)?;
            tx.commit()?;
            tracing::info!(version = target, "migration sqlite appliquee");
        }
    }
    Ok(())
}
