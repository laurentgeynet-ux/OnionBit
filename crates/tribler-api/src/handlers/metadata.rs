//! Handlers `/api/metadata` — equivalent de
//! `tribler.core.database.restapi.database_endpoint`.
//!
//! Source de verite : `channel_node` + `torrent_state` (`tribler-db`).

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Borne commune des listes (`max_ids`/`limit` Python).
const LIST_LIMIT: u32 = 200;

/// Ligne `channel_node` -> objet `torrent` de la reponse Python
/// (`TorrentMetadata.to_json`-equivalent).
fn row_json(row: &tribler_db::ChannelNodeRow) -> serde_json::Value {
    let mut infohash = hex::encode(&row.infohash);
    // Les canaux portent la cle publique, pas un infohash.
    if row.infohash.len() != 20 {
        infohash = String::new();
    }
    serde_json::json!({
        "infohash": infohash,
        "name": row.title,
        "length": row.size,
        "size": row.size,
        "category": serde_json::Value::Null,
        "num_seeders": serde_json::Value::Null,
        "num_leechers": serde_json::Value::Null,
        "last_tracker_check": serde_json::Value::Null,
        "updated": row.torrent_date,
        "status": row.status,
        "id": row.rowid,
        "origin_id": row.origin_id,
        "public_key": hex::encode(&row.public_key),
        "votes": row.xxx,
        "type": row.metadata_type,
        "tag_processor_version": row.tag_processor_version,
    })
}

/// `GET /api/metadata/torrents/{infohash}/health` — sante du swarm
/// (`get_torrent_health` ; `refresh=1` force un scrape immediat via
/// le torrent checker).
#[derive(Debug, Deserialize)]
pub struct HealthQuery {
    /// Force un scrape immediat (equivalent `checkhealth=1`).
    pub refresh: Option<String>,
    /// Timeout du scrape Python (`timeout`).
    pub timeout: Option<String>,
}

pub async fn get_torrent_health(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Query(q): Query<HealthQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ih_vec = tribler_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    let ih: [u8; 20] = ih_vec
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::bad_request("infohash hex attendu (20 octets)"))?;

    if q.refresh.as_deref() == Some("1") {
        if let Some(checker) = state.session.torrent_checker() {
            let trackers = state
                .session
                .db()
                .with(|c| tribler_db::health::trackers_of(c, &ih))
                .unwrap_or_default();
            for t in &trackers {
                let _ = checker.check_tracker(t, &[ih]).await;
            }
        }
    }

    let row = state
        .session
        .db()
        .with(|c| tribler_db::health::get_torrent_state(c, &ih))?;
    Ok(Json(match row {
        Some(r) => serde_json::json!({
            "infohash": infohash,
            "query": { "infohash": infohash },
            "health": {
                "seeders": r.seeders,
                "leechers": r.leechers,
                "infohash": infohash,
                "last_check": r.last_check,
            },
        }),
        None => serde_json::json!({
            "infohash": infohash,
            "query": { "infohash": infohash },
            "health": "checking",
        }),
    }))
}

/// `GET /api/metadata/torrents/popular` — torrents les plus sains
/// (`get_popular_torrents` : tri seeders desc).
pub async fn get_popular_torrents(
    State(state): State<AppState>,
    Query(q): Query<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = q
        .get("limit")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| v.as_u64())
        })
        .map(|n| n.min(LIST_LIMIT as u64) as u32)
        .unwrap_or(LIST_LIMIT);
    let rows = state.session.db().with(|c| {
        let mut stmt = c.prepare(
            "SELECT n.infohash, n.title, n.size, n.torrent_date, t.seeders, t.leechers,
                    t.last_check
             FROM channel_node n
             LEFT JOIN torrent_state t ON t.rowid = n.health_rowid
             WHERE n.metadata_type IN (300,400)
             ORDER BY COALESCE(t.seeders, 0) DESC, n.xxx DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok(serde_json::json!({
                "infohash": hex::encode(r.get::<_, Vec<u8>>(0)?),
                "name": r.get::<_, String>(1)?,
                "length": r.get::<_, i64>(2)?,
                "size": r.get::<_, i64>(2)?,
                "updated": r.get::<_, i64>(3)?,
                "num_seeders": r.get::<_, Option<i64>>(4)?,
                "num_leechers": r.get::<_, Option<i64>>(5)?,
                "last_tracker_check": r.get::<_, Option<i64>>(6)?,
            }))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    Ok(Json(serde_json::json!({ "results": rows })))
}

/// `GET /api/metadata/torrents/health` — historique de sante
/// (`get_torrent_health_history` : infohashes recurrents).
pub async fn get_torrent_health_history(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = state.session.db().with(|c| {
        let mut stmt = c.prepare(
            "SELECT infohash, seeders, leechers, last_check FROM torrent_state
             ORDER BY last_check DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([LIST_LIMIT], |r| {
            Ok(serde_json::json!({
                "infohash": hex::encode(r.get::<_, Vec<u8>>(0)?),
                "num_seeders": r.get::<_, i64>(1)?,
                "num_leechers": r.get::<_, i64>(2)?,
                "last_tracker_check": r.get::<_, i64>(3)?,
            }))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    Ok(Json(serde_json::json!({ "history": rows })))
}

/// `GET /api/metadata/search/local?fts_text=...` — recherche locale
/// dans `channel_node` (`local_search` : LIKE sur le titre).
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Texte de recherche (`fts_text` Python, recu tel quel).
    pub fts_text: Option<String>,
    /// Filtre sur le type de metadonnee.
    pub metadata_type: Option<String>,
    /// Limite de resultats.
    pub limit: Option<u32>,
}

pub async fn local_search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fts = q.fts_text.unwrap_or_default();
    if fts.is_empty() {
        return Err(ApiError::bad_request("fts_text parameter missing"));
    }
    let rows = state.session.db().with(|c| {
        tribler_db::channel::search_by_title(
            c,
            &format!("%{}%", fts.replace('%', "\\%")),
            q.limit.unwrap_or(LIST_LIMIT).min(LIST_LIMIT),
        )
    })?;
    let results: Vec<_> = rows.iter().map(row_json).collect();
    Ok(Json(serde_json::json!({
        "results": results,
        "first": 0,
        "last": results.len(),
        "sort_by": "HEALTH",
        "sort_desc": true,
        "txt_filter": fts,
        "hide_xxx": false,
    })))
}

/// `GET /api/metadata/search/completions` — suggestions de titres
/// (`completions` Python : titres correspondant au prefixe).
pub async fn completions(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fts = q.fts_text.unwrap_or_default();
    let rows = state.session.db().with(|c| {
        let mut stmt = c.prepare(
            "SELECT DISTINCT title FROM channel_node
             WHERE title LIKE ?1 AND title != '' LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![format!("{fts}%"), LIST_LIMIT], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    Ok(Json(serde_json::json!({ "completions": rows })))
}

/// `GET /api/metadata/search/vocabulary` — vocabulaire FTS connu
/// (Tribler retourne les langues FTS5 disponibles ; nous ne faisons
/// pas de FTS -> liste vide documentee).
pub async fn vocabulary() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "vocabularies": [] }))
}

/// Corps des endpoints tags (`{"tag": "..."}` Python).
#[derive(Debug, Deserialize)]
pub struct TagRequest {
    /// Tag a ajouter/retirer.
    pub tag: Option<String>,
    /// Pour `PATCH` : liste complete de remplacement.
    pub tags: Option<Vec<String>>,
}

/// `PUT /api/metadata/torrents/{ih}/tags` — ajoute un tag a
/// l'entree `channel_node` du torrent (colonne `tags`,
/// liste separee par des virgules comme le rendu Python).
pub async fn add_tag(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tag = req
        .tag
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("tag parameter missing"))?;
    let ih = tribler_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    let exists = state.session.db().with(|c| {
        let mut row = match tribler_db::channel::get_by_infohash(c, &ih)? {
            Some(r) => r,
            None => return Ok(false),
        };
        let mut tags: Vec<String> = row
            .tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !tags.iter().any(|t| t == &tag) {
            tags.push(tag.clone());
        }
        row.tags = tags.join(",");
        c.execute(
            "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
            rusqlite::params![row.tags, row.rowid],
        )?;
        Ok(true)
    })?;
    if !exists {
        return Err(ApiError::not_found(format!(
            "torrent inconnu dans la base de metadonnees: {infohash}"
        )));
    }
    Ok(Json(serde_json::json!({
        "infohash": infohash,
        "added": true,
        "tag": tag,
    })))
}

/// `DELETE /api/metadata/torrents/{ih}/tags` — retire un tag.
pub async fn remove_tag(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tag = req
        .tag
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("tag parameter missing"))?;
    let ih = tribler_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    state.session.db().with(|c| {
        if let Some(mut row) = tribler_db::channel::get_by_infohash(c, &ih)? {
            let tags: Vec<String> = row
                .tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != &tag)
                .collect();
            row.tags = tags.join(",");
            c.execute(
                "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
                rusqlite::params![row.tags, row.rowid],
            )?;
        }
        Ok(())
    })?;
    Ok(Json(serde_json::json!({
        "infohash": infohash,
        "removed": true,
        "tag": tag,
    })))
}

/// `PATCH /api/metadata/torrents/{ih}/tags` — remplace la liste des
/// tags (`{"tags": [...]}`).
pub async fn update_tags(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tags = req
        .tags
        .ok_or_else(|| ApiError::bad_request("tags parameter missing"))?;
    let ih = tribler_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    state.session.db().with(|c| {
        if let Some(row) = tribler_db::channel::get_by_infohash(c, &ih)? {
            c.execute(
                "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
                rusqlite::params![tags.join(","), row.rowid],
            )?;
        }
        Ok(())
    })?;
    Ok(Json(serde_json::json!({
        "infohash": infohash,
        "updated": true,
        "tags": tags,
    })))
}
