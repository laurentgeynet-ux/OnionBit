// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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

/// Version courante du schema de ce crate — **doit etre egal a
/// `MIGRATIONS.len()`** : un ecart fait refuser au demon la
/// reouverture de sa propre base (`SchemaTooNew` au redemarrage).
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

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
    metadata_type  INTEGER NOT NULL,            -- cf. onionbit_format::mdblob::types
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
    // v4 : trackers retires par `DELETE .../trackers` (une URL par
    // ligne). librqbit fusionne toujours les trackers de la source
    // (announce/magnet `tr`) avec `AddTorrentOptions::trackers` —
    // pour honorer le retrait au re-add, la session filtre les
    // trackers de la source par cette liste (equivalent de la
    // modification de `tdef.atp.trackers` Python).
    "
ALTER TABLE downloads ADD COLUMN removed_trackers TEXT;
",
    // v5 : items RSS decouverts par les watchers (`GET /api/rss` —
    // extension Rust : Tribler Python ne persiste que `rss/urls` et
    // `previous_entries` en memoire). `link` = URL `.torrent` de
    // l'entree ; `title`/`infohash` renseignes quand la resolution
    // a produit des metadonnees.
    "
CREATE TABLE rss_items (
    feed_url   TEXT NOT NULL,
    link       TEXT NOT NULL,
    title      TEXT,
    infohash   TEXT,
    first_seen INTEGER NOT NULL,                -- secondes Unix
    PRIMARY KEY (feed_url, link)
);
",
    // v6 : attributs de canal du `DownloadConfig` Python
    // (`channel_download` / `add_download_to_channel`) — conserves
    // par telechargement en attente du portage des canaux.
    "
ALTER TABLE downloads ADD COLUMN channel_download INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN add_download_to_channel INTEGER NOT NULL DEFAULT 0;
",
    // v7 : index de recherche plein-texte `FtsIndex` — port verbatim
    // de `sql_create_fts_table`/triggers de `store.py` (FTS5 sur le
    // titre de `channel_node`, tokenizer porter/unicode61, prefixes
    // 2..5 pour l'auto-completion). `content='channel_node'` fait de
    // la table un index externe alimente par les triggers ; le
    // `INSERT ... VALUES('rebuild')` backfill les lignes existantes.
    "
CREATE VIRTUAL TABLE IF NOT EXISTS FtsIndex USING FTS5
    (title, content='channel_node',
     prefix = '2 3 4 5',
     tokenize='porter unicode61 remove_diacritics 1');
CREATE TRIGGER IF NOT EXISTS fts_ai AFTER INSERT ON channel_node
BEGIN
    INSERT INTO FtsIndex(rowid, title) VALUES (new.rowid, new.title);
END;
CREATE TRIGGER IF NOT EXISTS fts_ad AFTER DELETE ON channel_node
BEGIN
    DELETE FROM FtsIndex WHERE rowid = old.rowid;
END;
CREATE TRIGGER IF NOT EXISTS fts_au AFTER UPDATE ON channel_node BEGIN
    DELETE FROM FtsIndex WHERE rowid = old.rowid;
    INSERT INTO FtsIndex(rowid, title) VALUES (new.rowid, new.title);
END;
INSERT INTO FtsIndex(FtsIndex) VALUES('rebuild');
",
    // v8 : index des requetes chaudes de `metadata_endpoint` —
    // `popular`/`local_search` filtrent `channel_node` par
    // `metadata_type` (300/400) et trient par `torrent_date` ;
    // `popular`/l'historique de sante filtrent/ordonnent
    // `torrent_state` par `has_data`/`last_check`. Sans index,
    // chaque requete etait un scan complet + tri.
    "
CREATE INDEX idx_channel_node_type_date ON channel_node(metadata_type, torrent_date);
CREATE INDEX idx_torrent_state_health ON torrent_state(has_data, last_check);
CREATE INDEX idx_torrent_state_last_check ON torrent_state(last_check);
",
    // v9 : cache de pairs IPv8 verifies — recharge dans `Network`
    // au demarrage pour un bootstrap quasi immediat sans attendre
    // la resolution DNS des noeuds d'amorcage ni un premier walk.
    // `address` = "ip:port" numerique ; `last_seen` en secondes Unix
    // sert a l'expiration (`prune`) et au tri de chargement.
    "
CREATE TABLE ipv8_peers (
    public_key  BLOB PRIMARY KEY,
    address     TEXT NOT NULL,
    last_seen   INTEGER NOT NULL,
    new_style   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_ipv8_peers_seen ON ipv8_peers(last_seen);
",
    // v10 : stores PEX des swarms caches (`TunnelCommunity.pex` —
    // `PexCommunity` reduite a ses donnees, cf. `pex.rs` cote tunnel).
    // `own=1` : `intro_points_for` (seeder_pk que l'on annonce soi-meme
    // apres `establish-intro` — on reste joignable comme point
    // d'introduction au redemarrage) ; `own=0` : points d'introduction
    // appris via PEX (`intro_points`, TTL 300 s filtre au chargement).
    // Ecriture par snapshot complet (`replace_all`), comme le cache de
    // pairs — la carte est petite et volatile.
    "
CREATE TABLE tunnel_pex (
    info_hash BLOB NOT NULL,
    own       INTEGER NOT NULL DEFAULT 0,
    peer_key  BLOB NOT NULL,
    seeder_pk BLOB NOT NULL,
    address   TEXT NOT NULL DEFAULT '',
    source    INTEGER NOT NULL DEFAULT 0,
    last_seen INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (info_hash, own, peer_key, seeder_pk)
);
",
    // v11 : guard nodes (ADR-0010) — premiers sauts persistants de la
    // TunnelCommunity. Identite = cle publique (un changement d'IP ne
    // change pas le guard) ; `position` conserve l'ordre du set
    // (actifs d'abord, reserve ensuite) qui est semantique pour
    // `order_first_hops`. Ecriture par snapshot complet (<= 5 lignes).
    "
CREATE TABLE guards (
    public_key  BLOB PRIMARY KEY,
    address     TEXT NOT NULL DEFAULT '',
    adopted_at  INTEGER NOT NULL,
    last_seen   INTEGER NOT NULL,
    failures    INTEGER NOT NULL DEFAULT 0,
    reserve     INTEGER NOT NULL DEFAULT 0,
    position    INTEGER NOT NULL DEFAULT 0
);
",
    // v12 : index `torrent_state(seeders)` — `healths_for` populaire
    // ordonne `ORDER BY t.seeders DESC` ; sans index c'est un tri
    // complet de la jointure a chaque cache-miss du cache applicatif.
    "
CREATE INDEX idx_torrent_state_seeders ON torrent_state(seeders);
",
    // v13 : cumuls tous-temps `all_time_upload`/`all_time_download`
    // (le checkpoint `DownloadConfig` Python persiste les memes
    // compteurs). Les compteurs librqbit sont par session : la boucle
    // de progression accumule les deltas observes a chaque tick.
    "
ALTER TABLE downloads ADD COLUMN total_uploaded INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN total_downloaded INTEGER NOT NULL DEFAULT 0;
",
    // v14 : les reponses de recherche distante et les santes de
    // gossip ne sont plus persistees (`process_select_response` /
    // `process_health` travaillent en memoire). Purge du catalogue
    // accumule : `channel_node` ne conserve que nos propres torrents
    // (`public_key` tout a zero, inseres a l'ajout d'un
    // telechargement pour les servir aux selects entrants) — les
    // triggers `fts_ad` vident `FtsIndex` — et `torrent_state` ne
    // garde que les santes de nos telechargements (le checker les
    // repeuplera).
    "
DELETE FROM channel_node WHERE public_key <> zeroblob(64);
-- `torrent_state` : on garde les santes de nos telechargements ET
-- les lignes encore referencees par `channel_node.health_rowid`
-- (nos propres entrees conservees ci-dessus) — sinon la suppression
-- violerait la contrainte FK.
DELETE FROM torrent_state_tracker WHERE torrent_state_rowid NOT IN
    (SELECT rowid FROM torrent_state
     WHERE infohash IN (SELECT infohash FROM downloads)
        OR rowid IN (SELECT health_rowid FROM channel_node
                     WHERE health_rowid IS NOT NULL));
DELETE FROM torrent_state WHERE infohash NOT IN
    (SELECT infohash FROM downloads)
   AND rowid NOT IN (SELECT health_rowid FROM channel_node
                     WHERE health_rowid IS NOT NULL);
",
    // v15 : messagerie ADR-0011 — contacts (consentement, seq
    // sortant, retention) et messages (direction, seq, statut de
    // livraison). Persistance en clair v1 (assume dans l'ADR) ;
    // `secure_delete` zeroise le corps avant le DELETE a l'expiration.
    "
CREATE TABLE msg_contacts (
    public_key     BLOB PRIMARY KEY,
    state          TEXT NOT NULL CHECK (state IN ('active','pending','blocked')),
    send_seq       INTEGER NOT NULL DEFAULT 0,
    recv_top       INTEGER NOT NULL DEFAULT 0,
    retention_secs INTEGER NOT NULL DEFAULT 0,
    secure_delete  INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE TABLE msg_messages (
    id          BLOB PRIMARY KEY,
    contact_pk  BLOB NOT NULL REFERENCES msg_contacts(public_key) ON DELETE CASCADE,
    direction   TEXT NOT NULL CHECK (direction IN ('in','out')),
    seq         INTEGER NOT NULL,
    ts          INTEGER NOT NULL,
    body        BLOB NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('received','sent','acked','failed')),
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_msg_messages_contact ON msg_messages(contact_pk, ts);
",
    // v16 : pseudonyme local des contacts messagerie — la liste
    // n'affichait que la cle publique hex (illisible). `''` = pas
    // de pseudonyme, le client retombe sur la cle abregee.
    "
ALTER TABLE msg_contacts ADD COLUMN alias TEXT NOT NULL DEFAULT '';
",
    // v17 : comptabilite locale par pair (ADR-0015) — octets servis
    // aux circuits joints par `create` direct vs octets transportes
    // par les sauts verifies de nos circuits. Identite = cle publique
    // (le pair peut changer d'IP, pas de cle). Flush par upsert des
    // seules lignes modifiees.
    "
CREATE TABLE peer_stats (
    public_key      BLOB PRIMARY KEY,
    bytes_served    INTEGER NOT NULL DEFAULT 0,
    bytes_used      INTEGER NOT NULL DEFAULT 0,
    circuits_served INTEGER NOT NULL DEFAULT 0,
    circuits_used   INTEGER NOT NULL DEFAULT 0,
    first_seen      INTEGER NOT NULL,
    last_seen       INTEGER NOT NULL
);
",
    // v18 : attestations de curation (ADR-0015 §6) — verdicts signes
    // Ed25519 des curateurs suivis sur des sujets (info-hash ou cle
    // de canal). Auto-portantes (curateur + signature dans la ligne)
    // et dedupliquees par (curateur, kind, sujet) : seule la plus
    // recente (`ts` max) compte — l'upsert n'ecrase que si `ts`
    // strictement plus recent (anti-replay d'un vieux verdict).
    "
CREATE TABLE attestations (
    curator     BLOB NOT NULL,
    kind        INTEGER NOT NULL,
    subject     BLOB NOT NULL,
    verdict     INTEGER NOT NULL,
    ts          INTEGER NOT NULL,
    signature   BLOB NOT NULL,
    added_on    INTEGER NOT NULL,
    PRIMARY KEY (curator, kind, subject)
);
CREATE INDEX idx_attestations_subject ON attestations(kind, subject);
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
