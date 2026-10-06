// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/metadata` — equivalent de
//! `tribler.core.database.restapi.database_endpoint`.
//!
//! Source de verite : `channel_node` + `torrent_state` (`onionbit-db`).

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Borne commune des listes (`max_ids`/`limit` Python).
const LIST_LIMIT: u32 = 200;

/// Ligne `channel_node` -> objet `torrent` de la reponse Python
/// (`TorrentMetadata.to_json`-equivalent).
fn row_json(row: &onionbit_db::ChannelNodeRow) -> serde_json::Value {
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
        // `health.seeders/leechers/last_check` joints (Pony
        // `TorrentMetadata.to_json`).
        "num_seeders": row.health_seeders,
        "num_leechers": row.health_leechers,
        "last_tracker_check": row.health_last_check,
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
    let ih_vec = onionbit_crypto::hash::from_hex(&infohash)
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
                .call("metadata.trackers_of", move |c| {
                    onionbit_db::health::trackers_of(c, &ih)
                })
                .await
                .unwrap_or_default();
            for t in &trackers {
                let _ = checker.check_tracker(t, &[ih]).await;
            }
        }
    }

    let row = state
        .session
        .db()
        .call("metadata.torrent_state", move |c| {
            onionbit_db::health::get_torrent_state(c, &ih)
        })
        .await?;
    // Repli memoire : `torrent_state` ne contient que nos
    // telechargements depuis v14 — les santes gossip connues
    // comblent le trou pour les resultats distants.
    let gossip = state.session.ipv8().and_then(|s| {
        s.content_discovery
            .as_ref()
            .and_then(|cd| cd.known_health(&ih))
    });
    Ok(Json(match (row, gossip) {
        (Some(r), _) => serde_json::json!({
            "infohash": infohash,
            "query": { "infohash": infohash },
            "health": {
                "seeders": r.seeders,
                "leechers": r.leechers,
                "infohash": infohash,
                "last_check": r.last_check,
            },
        }),
        (None, Some(g)) => serde_json::json!({
            "infohash": infohash,
            "query": { "infohash": infohash },
            "health": {
                "seeders": g.seeders,
                "leechers": g.leechers,
                "infohash": infohash,
                "last_check": g.last_check,
            },
        }),
        (None, None) => serde_json::json!({
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
    let rows = state
        .session
        .db()
        .call("metadata.popular", move |c| {
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
        })
        .await?;

    // Inclut les telechargements de la session courante si non encore dans channel_node.
    let mut rows = rows;
    for dl in state.session.downloads() {
        let name = dl.name.clone().unwrap_or_default();
        if !name.is_empty()
            && !rows
                .iter()
                .any(|r| r.get("infohash").and_then(|v| v.as_str()) == Some(&dl.info_hash))
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            rows.push(serde_json::json!({
                "infohash": dl.info_hash,
                "name": name,
                "length": dl.total_bytes,
                "size": dl.total_bytes,
                "updated": now,
                "num_seeders": 1,
                "num_leechers": dl.peers_live,
                "last_tracker_check": null,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "results": rows })))
}

/// `GET /api/metadata/torrents/health` — historique de sante
/// (`get_torrent_health_history` : infohashes recurrents).
pub async fn get_torrent_health_history(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = state
        .session
        .db()
        .call("metadata.health_history", |c| {
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
        })
        .await?;
    Ok(Json(serde_json::json!({ "history": rows })))
}

/// `GET /api/metadata/search/local?fts_text=...` — recherche locale
/// dans `channel_node` (`local_search` : requete augmentee par le
/// vocabulaire de sous-mots, `query_with_augmenter` Python).
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Texte de recherche (`fts_text` Python, recu tel quel).
    pub fts_text: Option<String>,
    /// Filtre additionnel ajoute au texte (`filter` Python).
    pub filter: Option<String>,
    /// `first` (defaut Python 1).
    pub first: Option<u64>,
    /// `last` (defaut Python 50).
    pub last: Option<u64>,
    /// `sort_by` (`json2pony_columns` : name/size/created/health/...).
    pub sort_by: Option<String>,
    /// `sort_desc` (defaut Python true, `parse_bool` tolere
    /// `"true"`/`"false"`).
    pub sort_desc: Option<String>,
    /// `hide_xxx` (defaut Python false).
    pub hide_xxx: Option<String>,
    /// `category` (filtre suffixe de tags).
    pub category: Option<String>,
    /// `tags` : valeur CSV (`?tags=a,b`) — les occurrences repetees
    /// `?tags=a&tags=b` sont fusionnees dans le handler.
    pub tags: Option<String>,
    /// `max_rowid` (pagination pushback).
    pub max_rowid: Option<i64>,
    /// `channel_pk` hex.
    pub channel_pk: Option<String>,
    /// `origin_id`.
    pub origin_id: Option<i64>,
    /// `metadata_type` (entier ou CSV d'entiers).
    pub metadata_type: Option<String>,
    /// `include_total` Python : renvoie `total` + `max_rowid`.
    pub include_total: Option<String>,
    /// Compatibilite avec l'ancien parametre `limit` (borne `last`).
    pub limit: Option<u64>,
}

/// `parse_bool` Python (`true`/`1`/`yes`…), defaut `d`.
fn parse_bool_opt(v: Option<&str>, d: bool) -> bool {
    match v.map(|s| s.to_ascii_lowercase()) {
        Some(s) => matches!(s.as_str(), "true" | "1" | "yes" | "on"),
        None => d,
    }
}

/// `fts_text`/`filter` combines du parametre de recherche Python.
fn combined_fts(q_fts: &Option<String>, q_filter: &Option<String>) -> Option<String> {
    let mut t = q_fts.clone().unwrap_or_default();
    if let Some(f) = q_filter {
        t.push(' ');
        t.push_str(f);
    }
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Parse `metadata_type` (entier ou liste CSV).
fn parse_metadata_types(v: &Option<String>) -> Option<Vec<i64>> {
    v.as_ref().map(|s| {
        s.split(',')
            .filter_map(|t| t.trim().parse::<i64>().ok())
            .collect::<Vec<_>>()
    })
}

/// Construit les `SelectParams` partages local/remote a partir des
/// parametres REST sanitizes (`sanitize_parameters` Python).
fn build_select_params(
    q: &SearchQuery,
    extra_tags: Vec<String>,
    fts: Option<String>,
) -> onionbit_db::channel::SelectParams {
    let mut tags = extra_tags;
    if let Some(csv) = &q.tags {
        tags.extend(
            csv.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
        );
    }
    let sort_by = q.sort_by.clone();
    onionbit_db::channel::SelectParams {
        txt_filter: fts
            .as_deref()
            .and_then(onionbit_core::queries::to_fts_query),
        terms: fts
            .as_deref()
            .map(onionbit_core::queries::fts_terms)
            .unwrap_or_default(),
        metadata_types: parse_metadata_types(&q.metadata_type),
        channel_pk: q.channel_pk.as_ref().and_then(|s| hex::decode(s).ok()),
        origin_id: q.origin_id,
        max_rowid: q.max_rowid,
        hide_xxx: parse_bool_opt(q.hide_xxx.as_deref(), false),
        sort_by,
        sort_desc: parse_bool_opt(q.sort_desc.as_deref(), true),
        category: q.category.clone(),
        tags,
        first: q.first.unwrap_or(1).max(1),
        last: Some(q.last.or(q.limit).unwrap_or(50)),
        ..Default::default()
    }
}

/// Parametres repetes `?tags=a&tags=b` (`parameters.getall` Python) —
/// `serde_urlencoded` ne les supporte pas : lecture directe de la
/// query string.
pub(crate) fn repeated_tags(raw_query: Option<&str>) -> Vec<String> {
    let Some(query) = raw_query else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| *k == "tags")
        .map(|(_, v)| {
            let v = v.replace('+', " ");
            percent_decode(&v)
        })
        .collect()
}

/// Decode `%XX` minimal d'une valeur de query string.
fn percent_decode(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn local_search(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(fts) = combined_fts(&q.fts_text, &q.filter) else {
        return Err(ApiError::bad_request("fts_text parameter missing"));
    };
    let tags = repeated_tags(raw.as_deref());
    let params = build_select_params(&q, tags, Some(fts.clone()));

    // `query_with_augmenter` Python : l'augmenteur decoupe la requete
    // en sous-mots appris et rend des rowids (`LIMIT/OFFSET` propres
    // a l'augmenteur), puis `apply_sort_by_option` trie les entrees.
    // `query_with_augmenter` Python : `limit = max(1, last-first)`,
    // `offset = max(1, first)`. L'`OFFSET` SQL est 0-base : `first=1`
    // doit commencer a la premiere ligne (sinon elle est sautee).
    let limit = (params.last.unwrap_or(50))
        .saturating_sub(params.first)
        .max(1) as usize;
    let offset = params.first.max(1).saturating_sub(1) as usize;
    let (aug_sql, aug_params) = state.session.augmenter().augment(&fts, limit, offset);
    let rowids: Vec<i64> = state
        .session
        .db()
        .call("metadata.augment", move |c| {
            let mut stmt = c.prepare(&aug_sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = aug_params
                .iter()
                .map(|p| p as &dyn rusqlite::ToSql)
                .collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(refs), |r| r.get::<_, i64>(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .await?;

    let mut p2 = params.clone();
    p2.txt_filter = None;
    p2.terms.clear();
    p2.rowids = rowids;
    p2.first = 1;
    p2.last = None;
    // L'augmenteur n'a retenu aucun rowid -> aucun resultat. Sans ce
    // garde-fou, `build_where` ne pose aucune clause et
    // `select_entries` retournerait toute la table (le filtre texte
    // a ete retire au profit de `rowids`).
    let rows = if p2.rowids.is_empty() {
        Vec::new()
    } else {
        state
            .session
            .db()
            .call("metadata.select_entries", move |c| {
                onionbit_db::channel::select_entries(c, &p2)
            })
            .await?
    };

    // `include_total` Python : compte sans pagination via la
    // branche FTS (`get_total_count`) + `get_max_rowid` — une seule
    // tache bloquante pour les deux requetes.
    let (total, max_rowid) = if parse_bool_opt(q.include_total.as_deref(), false) {
        let p3 = params.clone();
        state
            .session
            .db()
            .call("metadata.count_entries", move |c| {
                Ok((
                    onionbit_db::channel::count_entries(c, &p3)?,
                    onionbit_db::channel::max_rowid(c)?,
                ))
            })
            .await
            .map(|(t, m)| (Some(t), Some(m)))
            .unwrap_or((None, None))
    } else {
        (None, None)
    };
    let mut results: Vec<_> = rows.iter().map(row_json).collect();

    // Inclut egalement les telechargements de la session courante correspondant au filtre.
    let fts_lower = fts.to_ascii_lowercase();
    for dl in state.session.downloads() {
        let name = dl.name.clone().unwrap_or_default();
        if (name.to_ascii_lowercase().contains(&fts_lower)
            || dl.info_hash.eq_ignore_ascii_case(&fts))
            && !results
                .iter()
                .any(|r| r.get("infohash").and_then(|v| v.as_str()) == Some(&dl.info_hash))
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            results.push(serde_json::json!({
                "infohash": dl.info_hash,
                "name": name,
                "length": dl.total_bytes,
                "size": dl.total_bytes,
                "updated": now,
                "num_seeders": 1,
                "num_leechers": dl.peers_live,
                "last_tracker_check": null,
            }));
        }
    }

    // `local_query_results` Python (`database_endpoint.local_search` :
    // notification des resultats apres la recherche FTS).
    state
        .session
        .notifier()
        .notify(onionbit_core::Notification::LocalQueryResults {
            query: fts.clone(),
            results: results.clone(),
        });

    let mut resp = serde_json::json!({
        "results": results,
        "first": params.first,
        "last": params.last.unwrap_or(50),
        "sort_by": params.sort_by,
        "sort_desc": params.sort_desc,
    });
    if let (Some(t), Some(m)) = (total, max_rowid) {
        resp["total"] = serde_json::json!(t);
        resp["max_rowid"] = serde_json::json!(m);
    }
    Ok(Json(resp))
}

/// `GET /api/metadata/search/completions?q=...` — suggestions de
/// titres (`completions` Python : `get_auto_complete_terms`, requete
/// FTS prefixe `"{mots}"*` + continuation regex `\W+`).
#[derive(Debug, Deserialize)]
pub struct CompletionsQuery {
    /// Texte partiel (`q` Python).
    pub q: Option<String>,
    /// Limite (non utilisee cote Python — `max_terms` fixe).
    pub max_terms: Option<usize>,
}

pub async fn completions(
    State(state): State<AppState>,
    Query(q): Query<CompletionsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // `fts_keyword_search_re.findall` Python.
    let words = onionbit_core::queries::fts_terms(&q.q.unwrap_or_default());
    let max_terms = q.max_terms.unwrap_or(5).max(1);
    let suggestions = state
        .session
        .db()
        .call("metadata.completions", move |c| {
            onionbit_db::channel::autocomplete_terms(c, &words, max_terms)
        })
        .await
        .unwrap_or_default();
    Ok(Json(serde_json::json!({ "completions": suggestions })))
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
    let ih = onionbit_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    let tag2 = tag.clone();
    let exists = state
        .session
        .db()
        .call("metadata.add_tag", move |c| {
            let mut row = match onionbit_db::channel::get_by_infohash(c, &ih)? {
                Some(r) => r,
                None => return Ok(false),
            };
            let mut tags: Vec<String> = row
                .tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !tags.iter().any(|t| t == &tag2) {
                tags.push(tag2.clone());
            }
            row.tags = tags.join(",");
            c.execute(
                "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
                rusqlite::params![row.tags, row.rowid],
            )?;
            Ok(true)
        })
        .await?;
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
    let ih = onionbit_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    let tag2 = tag.clone();
    state
        .session
        .db()
        .call("metadata.remove_tag", move |c| {
            if let Some(mut row) = onionbit_db::channel::get_by_infohash(c, &ih)? {
                let tags: Vec<String> = row
                    .tags
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && s != &tag2)
                    .collect();
                row.tags = tags.join(",");
                c.execute(
                    "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
                    rusqlite::params![row.tags, row.rowid],
                )?;
            }
            Ok(())
        })
        .await?;
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
    let ih = onionbit_crypto::hash::from_hex(&infohash)
        .ok_or_else(|| ApiError::bad_request("infohash hex attendu"))?;
    let tags_csv = tags.join(",");
    state
        .session
        .db()
        .call("metadata.update_tags", move |c| {
            if let Some(row) = onionbit_db::channel::get_by_infohash(c, &ih)? {
                c.execute(
                    "UPDATE channel_node SET tags = ?1 WHERE rowid = ?2",
                    rusqlite::params![tags_csv, row.rowid],
                )?;
            }
            Ok(())
        })
        .await?;
    Ok(Json(serde_json::json!({
        "infohash": infohash,
        "updated": true,
        "tags": tags,
    })))
}
