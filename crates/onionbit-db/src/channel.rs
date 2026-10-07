// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
        // Colonnes presentes seulement quand la requete joint
        // `torrent_state` (`select_filtered`/`popular_entries`).
        health_seeders: r.get::<_, Option<i64>>("seeders").unwrap_or_default(),
        health_leechers: r.get::<_, Option<i64>>("leechers").unwrap_or_default(),
        health_last_check: r.get::<_, Option<i64>>("last_check").unwrap_or_default(),
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

/// `POPULAR_TORRENTS_FRESHNESS_PERIOD` Python : fenetre des torrents
/// « populaires » (dernier jour, sante verifiee).
pub const POPULAR_TORRENTS_FRESHNESS_PERIOD: i64 = 60 * 60 * 24;
/// `POPULAR_TORRENTS_COUNT` Python.
pub const POPULAR_TORRENTS_COUNT: i64 = 100;

/// `json2pony_columns` Python : noms de `sort_by` de l'API vers les
/// colonnes `channel_node`. `"HEALTH"` est un pseudo-champ trie sur la
/// sante jointe (`torrent_state.seeders/leechers`).
fn sort_by_column(name: &str) -> Option<&'static str> {
    Some(match name {
        "category" => "tags",
        "name" => "title",
        "size" => "size",
        "infohash" => "infohash",
        "date" | "created" => "torrent_date",
        "status" => "status",
        // `votes`/`subscribed` Python sont des attributs Pony de la
        // table channels — pas de colonne correspondante ici : repli
        // sur le tri par defaut plutot qu'une erreur SQL.
        "health" => "HEALTH",
        _ => return None,
    })
}

/// Parametres d'un select distant/local (`get_entries` Python,
/// `sanitize_query`) pour [`select_entries`].
#[derive(Debug, Default, Clone)]
pub struct SelectParams {
    /// `txt_filter` : requete FTS5 deja formatee (`"a" "b"` — sortie
    /// de `to_fts_query`) passee a `FtsIndex MATCH`. Repli `LIKE` via
    /// [`Self::terms`] si l'index est absent ou la syntaxe invalide.
    pub txt_filter: Option<String>,
    /// Termes extraits du `txt_filter` (repli `LIKE` AND).
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
    /// `id_` (entree precise d'un canal).
    pub id_: Option<i64>,
    /// `max_rowid` : `rowid <= max_rowid` (pagination pushback Python).
    pub max_rowid: Option<i64>,
    /// `hide_xxx` Python : exclut les entrees marquee xxx
    /// (`xxx >= 0.5` est le seuil du classifieur Tribler).
    pub hide_xxx: bool,
    /// `sort_by` brut de l'API (mappe par [`sort_by_column`]).
    pub sort_by: Option<String>,
    /// `sort_desc` Python (defaut true).
    pub sort_desc: bool,
    /// `category` : filtre suffixe de `tags`
    /// (`tags.endswith(cat) or (cat+",") in tags` Python).
    pub category: Option<String>,
    /// `tags` : meme filtre par tag (conjonction).
    pub tags: Vec<String>,
    /// `self_checked_torrent` : filtre `torrent_state.self_checked`.
    pub self_checked: Option<bool>,
    /// `health_checked_after` : `has_data=1 AND last_check >=`.
    pub health_checked_after: Option<i64>,
    /// `popular` : sous-requete des torrents sains recents (exige
    /// `metadata_type == REGULAR_TORRENT`, comme Python).
    pub popular: bool,
    /// Pagination Python : `first` (defaut 1) / `last` — tranche
    /// `[first-1..last]` comme `pony_query[first-1:last]`.
    pub first: u64,
    /// `last` (borne incluse Python).
    pub last: Option<u64>,
    /// Restreint aux `rowid` listes — chemin de la recherche
    /// augmentee (`query_with_augmenter` Python : l'augmenteur rend
    /// des rowids, puis `apply_sort_by_option` trie).
    pub rowids: Vec<i64>,
}

impl SelectParams {
    /// Expression `ORDER BY` — `apply_sort_by_option` Python :
    /// `sort_desc` sur `rowid` par defaut ; `HEALTH` trie sur
    /// `seeders`/`leechers` joints ; colonne nommee en `COLLATE
    /// NOCASE`.
    fn order_by(&self, txt_filter: bool, args: &mut Vec<Box<dyn rusqlite::ToSql>>) -> String {
        let dir = if self.sort_desc { "DESC" } else { "ASC" };
        match self.sort_by.as_deref().and_then(sort_by_column) {
            Some("HEALTH") => format!("ts.seeders {dir}, ts.leechers {dir}"),
            Some(col) => format!("cn.{col} COLLATE NOCASE {dir}"),
            None if txt_filter => {
                // Tri de pertinence `sort_by is None and txt_filter` :
                // canaux d'abord, puis `search_rank` (UDF
                // `torrent_rank`), puis fraicheur du health check.
                args.push(Box::new(self.txt_filter.clone().unwrap_or_default()));
                "CASE cn.metadata_type
                     WHEN 400 THEN 1 WHEN 220 THEN 2 ELSE 3 END,
                 search_rank(?, cn.title, ts.seeders, ts.leechers,
                     CAST(strftime('%s','now') AS INTEGER) - cn.torrent_date) DESC,
                 ts.last_check DESC"
                    .to_string()
            }
            None => format!("cn.rowid {dir}"),
        }
    }
}

/// `MetadataStore.get_entries` Python : `channel_node` filtre par les
/// parametres d'un select (`txt_filter` -> `FtsIndex MATCH`,
/// `infohash`/`infohash_set`, `channel_pk`, `origin_id`, `id_`,
/// `metadata_type`, `max_rowid`, `hide_xxx`, `category`/`tags`,
/// `sort_by`/`sort_desc`), pagination `first..last`.
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
    if p.popular {
        return popular_entries(conn);
    }
    select_filtered(conn, p)
}

/// `get_total_count` : meme requete sans pagination ni tri.
pub fn count_entries(conn: &Connection, p: &SelectParams) -> Result<i64> {
    if p.popular {
        return Ok(popular_entries(conn)?.len() as i64);
    }
    let (where_sql, args, _fts) = build_where(conn, p);
    let sql = format!(
        "SELECT COUNT(*) FROM channel_node cn
         LEFT JOIN torrent_state ts ON ts.rowid = cn.health_rowid{where_sql}"
    );
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(refs), |r| r.get(0))?;
    Ok(n)
}

/// `get_max_rowid` Python.
pub fn max_rowid(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM channel_node",
        [],
        |r| r.get(0),
    )?)
}

/// Clause `WHERE` commune a `select_entries`/`count_entries` :
/// retourne `(sql, args, use_fts)`.
fn build_where(
    conn: &Connection,
    p: &SelectParams,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>, bool) {
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut parts: Vec<String> = Vec::new();

    let use_fts = p
        .txt_filter
        .as_ref()
        .is_some_and(|f| !f.is_empty() && fts_available(conn));
    if use_fts {
        // `search_keyword` Python : `rowid IN (SELECT rowid FROM
        // FtsIndex WHERE FtsIndex MATCH ?)`.
        parts.push("cn.rowid IN (SELECT rowid FROM FtsIndex WHERE FtsIndex MATCH ?)".into());
        args.push(Box::new(p.txt_filter.clone().unwrap_or_default()));
    } else {
        for t in &p.terms {
            parts.push("cn.title LIKE ?".into());
            args.push(Box::new(format!("%{t}%")));
        }
    }
    if let Some(mr) = p.max_rowid {
        parts.push("cn.rowid <= ?".into());
        args.push(Box::new(mr));
    }
    if !p.rowids.is_empty() {
        parts.push(format!(
            "cn.rowid IN ({})",
            p.rowids.iter().map(|_| "?").collect::<Vec<_>>().join(",")
        ));
        for r in &p.rowids {
            args.push(Box::new(*r));
        }
    }
    if let Some(mts) = &p.metadata_types {
        if !mts.is_empty() {
            parts.push(format!(
                "cn.metadata_type IN ({})",
                mts.iter().map(|_| "?").collect::<Vec<_>>().join(",")
            ));
            for m in mts {
                args.push(Box::new(*m));
            }
        }
    }
    if let Some(pk) = &p.channel_pk {
        parts.push("cn.public_key = ?".into());
        args.push(Box::new(pk.clone()));
    }
    if let Some(oid) = p.origin_id {
        parts.push("cn.origin_id = ?".into());
        args.push(Box::new(oid));
    }
    if let Some(id) = p.id_ {
        parts.push("cn.id_ = ?".into());
        args.push(Box::new(id));
    }
    // `category`/`tags` : `tags.endswith(t) OR (t+",") IN tags`.
    for t in p.category.iter().chain(p.tags.iter()) {
        parts.push("(cn.tags LIKE ? OR instr(cn.tags, ?) > 0)".into());
        args.push(Box::new(format!("%{t}")));
        args.push(Box::new(format!("{t},")));
    }
    if p.hide_xxx {
        parts.push("cn.xxx = 0".into());
    }
    if let Some(sc) = p.self_checked {
        parts.push("ts.self_checked = ?".into());
        args.push(Box::new(i64::from(sc)));
    }
    if let Some(hca) = p.health_checked_after {
        parts.push("ts.has_data = 1 AND ts.last_check >= ?".into());
        args.push(Box::new(hca));
    }
    let sql = if parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", parts.join(" AND "))
    };
    (sql, args, use_fts)
}

/// Corps de `select_entries` (`get_entries` Python).
fn select_filtered(conn: &Connection, p: &SelectParams) -> Result<Vec<ChannelNodeRow>> {
    let (where_sql, mut args, use_fts) = build_where(conn, p);
    let cols = COLS
        .split(',')
        .map(|c| format!("cn.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT {cols}, ts.seeders, ts.leechers, ts.last_check
         FROM channel_node cn
         LEFT JOIN torrent_state ts ON ts.rowid = cn.health_rowid{where_sql}"
    );
    sql.push_str(&format!(
        " ORDER BY {}",
        p.order_by(use_fts || !p.terms.is_empty(), &mut args)
    ));
    // `pony_query[first-1:last]` : OFFSET first-1, LIMIT borne.
    let first = p.first.max(1);
    let last = p.last.unwrap_or(first + 49);
    sql.push_str(" LIMIT ? OFFSET ?");
    args.push(Box::new(
        i64::try_from(last.saturating_sub(first) + 1).unwrap_or(i64::MAX),
    ));
    args.push(Box::new(i64::try_from(first - 1).unwrap_or(i64::MAX)));

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(refs), from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// La table `FtsIndex` existe-t-elle (migration v7 appliquee) ?
fn fts_available(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='FtsIndex'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// Branche `popular` de `get_entries_query` Python : les
/// `POPULAR_TORRENTS_COUNT` torrents sains vus recemment, dedupes par
/// infohash.
fn popular_entries(conn: &Connection) -> Result<Vec<ChannelNodeRow>> {
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        - POPULAR_TORRENTS_FRESHNESS_PERIOD;
    let cols = COLS
        .split(',')
        .map(|c| format!("cn.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT {cols}, results.seeders, results.leechers, results.last_check FROM
           (SELECT * FROM torrent_state
            WHERE has_data = 1 AND last_check >= ?1
              AND (seeders > 0 OR leechers > 0)
            ORDER BY seeders DESC, leechers DESC, last_check DESC
            LIMIT {POPULAR_TORRENTS_COUNT}) results
         INNER JOIN channel_node cn ON cn.health_rowid = results.rowid
         GROUP BY cn.infohash"
    ))?;
    let rows = stmt.query_map(params![cutoff], from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// `get_auto_complete_terms` Python : titres dont le prefixe FTS
/// `"{mots}"*` correspond, ordonnes par seeders, puis extraction de la
/// continuation via le motif `\W+`-separe (suggestion suffixe).
pub fn autocomplete_terms(
    conn: &Connection,
    words: &[String],
    max_terms: usize,
) -> Result<Vec<String>> {
    if words.is_empty() || !fts_available(conn) {
        return Ok(Vec::new());
    }
    let fts_query = format!("\"{}\"*", words.join(" "));
    let mut stmt = conn.prepare(
        "SELECT cn.title FROM channel_node cn
         LEFT JOIN torrent_state ts ON cn.health_rowid = ts.rowid
         WHERE cn.rowid IN (
             SELECT rowid FROM FtsIndex WHERE FtsIndex MATCH ?1
             ORDER BY rowid DESC LIMIT ?2)
         ORDER BY COALESCE(ts.seeders, 0) DESC",
    )?;
    let titles = stmt.query_map(params![fts_query, max_terms.max(1) as i64], |r| {
        r.get::<_, String>(0)
    })?;
    let titles: Vec<String> = titles.collect::<std::result::Result<_, _>>()?;

    // `suggestion_re` Python : les mots joints par `\W+`, suivis de
    // `(\W*)((?:[.-]?\w)*)` — groupe 1 = separateurs apres le dernier
    // mot, groupe 2 = continuation du dernier mot (ou mot suivant).
    let mut result = Vec::new();
    let text = words.join(" ");
    let ends_word = text
        .chars()
        .last()
        .is_some_and(|c| c.is_alphanumeric() || c == '_');
    for title in titles {
        let lower = title.to_lowercase();
        if let Some((g1, g2)) = continuation(&lower, words) {
            let cont = if ends_word && !g1.is_empty() {
                format!("{g1}{g2}")
            } else {
                g2
            };
            let suggestion = format!("{text}{cont}");
            if !result.contains(&suggestion) {
                result.push(suggestion);
                if result.len() >= max_terms {
                    break;
                }
            }
        }
    }
    Ok(result)
}

/// Recherche `words` separés par des runs de non-lettres dans le
/// titre (le `suggestion_re` Python) et retourne `(\W+`, `[.-]?\w*`
/// suivants) = `(g1, g2)`.
fn continuation(title_lower: &str, words: &[String]) -> Option<(String, String)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<char> = title_lower.chars().collect();
    let mut pos = 0usize;
    for (wi, w) in words.iter().enumerate() {
        let wl: Vec<char> = w.to_lowercase().chars().collect();
        let mut found = None;
        let mut i = pos;
        while i + wl.len() <= chars.len() {
            // Premier mot : `re.search` autorise n'importe quel debut.
            // Suivants : un run `\W+` non vide separe du mot precedent.
            if chars[i..i + wl.len()] == wl[..]
                && (wi == 0 || (i > pos && chars[pos..i].iter().all(|c| !is_word(*c))))
            {
                found = Some(i);
                break;
            }
            i += 1;
        }
        let start = found?;
        pos = start + wl.len();
    }
    // g1 : run de non-lettres apres le dernier mot ; g2 : run de
    // `[.-]?\w` suivant.
    let g1_start = pos;
    while pos < chars.len() && !is_word(chars[pos]) && chars[pos] != '.' && chars[pos] != '-' {
        pos += 1;
    }
    let g1: String = chars[g1_start..pos].iter().collect();
    let g2_start = pos;
    if pos < chars.len() && (chars[pos] == '.' || chars[pos] == '-') {
        pos += 1;
    }
    while pos < chars.len() && is_word(chars[pos]) {
        pos += 1;
    }
    let g2: String = chars[g2_start..pos].iter().collect();
    Some((g1, g2))
}
