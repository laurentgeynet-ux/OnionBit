// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/statistics` — equivalent de
//! `tribler.core.restapi.statistics_endpoint`.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/statistics/tribler` — statistiques generales
/// (`tribler_statistics` : `db_size`, `num_torrents`, `peers`,
/// `libtorrent.sessions`, `endpoint_version`).
pub async fn get_onionbit_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_path = state.session.config().db_path();
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let (num_torrents, num_channels) = state
        .session
        .db()
        .call("statistics.counts", |c| {
            let torrents: i64 = c.query_row(
                "SELECT COUNT(*) FROM channel_node WHERE metadata_type IN (300,400)",
                [],
                |r| r.get(0),
            )?;
            let channels: i64 = c.query_row(
                "SELECT COUNT(DISTINCT public_key) FROM channel_node WHERE metadata_type = 400",
                [],
                |r| r.get(0),
            )?;
            Ok((torrents, channels))
        })
        .await
        .unwrap_or((0, 0));

    let mut stats = serde_json::json!({
        "db_size": db_size,
        "num_torrents": num_torrents,
        "num_channels": num_channels,
        "torrent_queue_stats": [],
        "endpoint_version": env!("CARGO_PKG_VERSION"),
    });

    if let Some(stack) = state.session.ipv8() {
        stats["peers"] = serde_json::json!(stack.discovery.peer_count());
        // Sessions libtorrent-equivalentes : moteur principal +
        // lanes anonymes (par nombre de sauts).
        let mut sessions = vec![serde_json::json!({"hops": 0})];
        for (hops, _addr) in stack.anon_lanes() {
            sessions.push(serde_json::json!({"hops": hops}));
        }
        stats["libtorrent"] = serde_json::json!({
            "sessions": sessions,
            "total_recv_bytes": serde_json::Value::Null,
            "total_sent_bytes": serde_json::Value::Null,
        });
        if stack.tunnel.is_some() {
            stats["socks5_sessions"] = serde_json::json!(stack
                .anon_lanes()
                .iter()
                .map(|(h, a)| serde_json::json!({
                    "hops": h, "listen": a.to_string(),
                }))
                .collect::<Vec<_>>());
        }
    }
    Json(serde_json::json!({ "tribler_statistics": stats }))
}

/// `GET /api/statistics/ipv8` — compteurs d'octets de l'endpoint
/// (`total_up`/`total_down`).
pub async fn get_ipv8_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let stats = match state.session.ipv8() {
        Some(stack) => {
            let (up, down) = stack.endpoint.bytes_counters();
            serde_json::json!({ "total_up": up, "total_down": down })
        }
        None => serde_json::json!({}),
    };
    Json(serde_json::json!({ "ipv8_statistics": stats }))
}

/// `GET /api/statistics/dirspace?path=...` — variante de confort de
/// la route Python (voir `put_dirspace_stats`).
#[derive(Debug, Deserialize)]
pub struct DirspaceQuery {
    /// Repertoire a mesurer.
    pub path: Option<String>,
}

pub async fn get_dirspace_stats(
    Query(q): Query<DirspaceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dirspace_response(
        q.path
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from),
    )
}

/// Corps de `PUT /api/statistics/dirspace` (`DirspaceStatsRequestBody`
/// Python : `{"directory": "..."}` — sans `directory`, le Python
/// retombe sur `libtorrent/download_defaults/saveas`).
#[derive(Debug, Deserialize)]
pub struct DirspaceBody {
    /// Repertoire a mesurer.
    pub directory: Option<String>,
}

/// `PUT /api/statistics/dirspace` — route Python exacte
/// (`web.put("/dirspace", ...)` dans `statistics_endpoint.py`) :
/// `{"statistics": {"total","used","free"}}` du premier ancetre
/// existant du repertoire demande.
pub async fn put_dirspace_stats(
    axum::Json(body): axum::Json<DirspaceBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dirspace_response(
        body.directory
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from),
    )
}

/// Fidele a `get_dirspace_stats` Python : `shutil.disk_usage` sur le
/// premier ancetre existant du chemin (404 "No stats for directory!"
/// si aucun n'existe).
fn dirspace_response(dir: Option<std::path::PathBuf>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut path = Some(dir.as_path());
    while let Some(p) = path {
        if let Ok(total) = fs2::total_space(p) {
            let free = fs2::free_space(p).unwrap_or(0);
            return Ok(Json(serde_json::json!({
                "statistics": {
                    "total": total,
                    "used": total.saturating_sub(free),
                    "free": free,
                }
            })));
        }
        path = p.parent();
    }
    Err(ApiError::not_found("No stats for directory!"))
}
