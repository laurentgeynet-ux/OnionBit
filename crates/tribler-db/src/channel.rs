//! Acces a `channel_node` : metadonnees de canaux/torrents
//! (equivalent des acces Pony de `TorrentMetadata`).

use rusqlite::{params, Connection, OptionalExtension};

use crate::health;
use crate::models::ChannelNodeRow;
use crate::Result;

const COLS: &str = "rowid, infohash, size, torrent_date, tracker_info, title,
                    tags, metadata_type, reserved_flags, origin_id, public_key,
                    id_, timestamp, signature, added_on, status, xxx,
                    health_rowid, tag_processor_version";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChannelNodeRow> {
    Ok(ChannelNodeRow {
        rowid: r.get("rowid")?,
        infohash: r.get("infohash")?,
        size: r.get("size")?,
        torrent_date: r.get("torrent_date")?,
        tracker_info: r.get("tracker_info")?,
        title: r.get("title")?,
        tags: r.get("tags")?,
        metadata_type: r.get("metadata_type")?,
        reserved_flags: r.get("reserved_flags")?,
        origin_id: r.get("origin_id")?,
        public_key: r.get("public_key")?,
        id_: r.get("id_")?,
        timestamp: r.get("timestamp")?,
        signature: r.get("signature")?,
        added_on: r.get("added_on")?,
        status: r.get("status")?,
        xxx: r.get("xxx")?,
        health_rowid: r.get("health_rowid")?,
        tag_processor_version: r.get("tag_processor_version")?,
    })
}

/// Insere une entree de canal. Si `infohash` est non vide, une entree
/// `torrent_state` est creee/liee automatiquement (comme le fait le
/// `__init__` de `TorrentMetadata` cote Python).
///
/// Retourne le rowid insere, ou `None` si la combinaison
/// `(public_key, id_)` existe deja (deduplication).
pub fn insert(conn: &Connection, row: &ChannelNodeRow) -> Result<Option<i64>> {
    let health_rowid = if row.infohash.is_empty() {
        row.health_rowid
    } else {
        Some(health::upsert_torrent_state(conn, &row.infohash)?.rowid)
    };
    let n = conn.execute(
        "INSERT INTO channel_node(
            infohash, size, torrent_date, tracker_info, title, tags,
            metadata_type, reserved_flags, origin_id, public_key, id_,
            timestamp, signature, added_on, status, xxx, health_rowid,
            tag_processor_version
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
         ON CONFLICT(public_key, id_) DO NOTHING",
        params![
            row.infohash,
            row.size,
            row.torrent_date,
            row.tracker_info,
            row.title,
            row.tags,
            row.metadata_type,
            row.reserved_flags,
            row.origin_id,
            row.public_key,
            row.id_,
            row.timestamp,
            row.signature,
            row.added_on,
            row.status,
            row.xxx,
            health_rowid,
            row.tag_processor_version,
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(conn.last_insert_rowid()))
}

/// Cherche une entree par info-hash (`get_with_infohash`).
pub fn get_by_infohash(conn: &Connection, infohash: &[u8]) -> Result<Option<ChannelNodeRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM channel_node WHERE infohash = ?1 LIMIT 1"),
            params![infohash],
            from_row,
        )
        .optional()?)
}

/// Cherche une entree par (public_key, id_).
pub fn get_by_pk_id(
    conn: &Connection,
    public_key: &[u8],
    id_: i64,
) -> Result<Option<ChannelNodeRow>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM channel_node
                 WHERE public_key = ?1 AND id_ = ?2 LIMIT 1"
            ),
            params![public_key, id_],
            from_row,
        )
        .optional()?)
}

/// Liste les entrees d'un canal (`public_key` = cle du canal),
/// les plus recentes d'abord.
pub fn channel_entries(
    conn: &Connection,
    channel_public_key: &[u8],
    limit: u32,
) -> Result<Vec<ChannelNodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM channel_node
         WHERE public_key = ?1 AND metadata_type <> 500
         ORDER BY timestamp DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![channel_public_key, limit], from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Recherche par titre (moteur simple pour la recherche Tribler).
///
/// `pattern` est un pattern SQL `LIKE` (avec `%` fournis par l'appelant).
pub fn search_by_title(
    conn: &Connection,
    pattern: &str,
    limit: u32,
) -> Result<Vec<ChannelNodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM channel_node
         WHERE title LIKE ?1 AND metadata_type IN (300, 400)
         ORDER BY torrent_date DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![pattern, limit], from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Parametres d'un select distant/local (`get_entries` Python,
/// `sanitize_query`) pour [`select_entries`].
#[derive(Debug, Default, Clone)]
pub struct SelectParams {
    /// Termes `txt_filter` (FTS `AND` : le titre doit contenir
    /// CHAQUE terme — approximation `LIKE` de `FtsIndex MATCH`).
    pub terms: Vec<String>,
    /// `metadata_type` accepte un entier ou une liste ; `None` =
    /// tous types.
    pub metadata_types: Option<Vec<i64>>,
    /// `infohash` unique (binaire).
    pub infohash: Option<Vec<u8>>,
    /// `infohash_set` (binaires).
    pub infohash_set: Vec<Vec<u8>>,
    /// `channel_pk` -> colonne `public_key`.
    pub channel_pk: Option<Vec<u8>>,
    /// `origin_id` (exige `channel_pk` cote Python — filtre simple ici).
    pub origin_id: Option<i64>,
    /// `max_rowid` : exclut les `rowid >=` (pagination pushback).
    pub max_rowid: Option<i64>,
    /// `hide_xxx` Python : exclut les entrees marquee xxx
    /// (`xxx >= 0.5` est le seuil du classifieur Tribler).
    pub hide_xxx: bool,
    /// Borne haute SQL (le `first`/`last` est tranche ensuite).
    pub limit: usize,
}

/// `MetadataStore.get_entries` Python : `channel_node` filtre par les
/// parametres d'un select (`txt_filter` -> AND de `LIKE`,
/// `infohash`/`infohash_set`, `channel_pk`, `origin_id`,
/// `metadata_type`, `max_rowid`, `hide_xxx`), tri `torrent_date DESC`.
pub fn select_entries(conn: &Connection, p: &SelectParams) -> Result<Vec<ChannelNodeRow>> {
    if let Some(ih) = &p.infohash {
        return Ok(get_by_infohash(conn, ih)?.into_iter().collect());
    }
    if !p.infohash_set.is_empty() {
        let mut out = Vec::new();
        for ih in &p.infohash_set {
            if let Some(row) = get_by_infohash(conn, ih)? {
                out.push(row);
            }
        }
        return Ok(out);
    }

    let mut sql = format!("SELECT {COLS} FROM channel_node WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for t in &p.terms {
        sql.push_str(" AND title LIKE ?");
        args.push(Box::new(format!("%{t}%")));
    }
    if let Some(mts) = &p.metadata_types {
        if !mts.is_empty() {
            sql.push_str(&format!(
                " AND metadata_type IN ({})",
                mts.iter().map(|_| "?").collect::<Vec<_>>().join(",")
            ));
            for m in mts {
                args.push(Box::new(*m));
            }
        }
    }
    if let Some(pk) = &p.channel_pk {
        sql.push_str(" AND public_key = ?");
        args.push(Box::new(pk.clone()));
    }
    if let Some(oid) = p.origin_id {
        sql.push_str(" AND origin_id = ?");
        args.push(Box::new(oid));
    }
    if let Some(mr) = p.max_rowid {
        sql.push_str(" AND rowid < ?");
        args.push(Box::new(mr));
    }
    if p.hide_xxx {
        sql.push_str(" AND xxx < 0.5");
    }
    sql.push_str(" ORDER BY torrent_date DESC LIMIT ?");
    args.push(Box::new(p.limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(refs), from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
