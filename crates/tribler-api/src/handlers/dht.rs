//! Handlers `/api/ipv8/dht` — equivalent de
//! `ipv8.REST.dht_endpoint` (pyipv8).
//!
//! Formes d'erreur fideles au Python : `{"success": false,
//! "error": ...}` (Response brute, pas `{error: {handled}}`) pour les
//! erreurs anticipees ; `{"error": {"handled": false, "message"}}` en
//! 500 pour les exceptions non gerees (`unhexlify` sur hex invalide,
//! `DHTError` de lookup) via `error_middleware`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{body::Bytes, Json};

use base64::Engine;
use tribler_ipv8::dht::{DhtCommunity, DhtError};

use crate::state::AppState;

/// `Response({"success": false, "error": ...}, status=404)` des
/// routes `statistics`/`values`/`peers` quand la community est
/// absente (`dht is None` Python).
fn dht_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "success": false,
            "error": "DHT community not found"
        })),
    )
        .into_response()
}

/// `refresh_bucket` a un message legerement different et un 400.
fn dht_not_loaded() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "success": false,
            "error": "DHT community is not loaded"
        })),
    )
        .into_response()
}

/// Exception non geree Python (`unhexlify`, `DHTError`) →
/// `error_middleware` : 500 `{"error": {"handled": false, ...}}`.
fn unhandled(message: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": { "handled": false, "message": message }
        })),
    )
        .into_response()
}

/// `unhexlify` Python : erreur → exception → 500 (geree par le
/// `unhandled` des call-sites ; le `Err(())` evite de transporter
/// une `Response` complete dans le Result). La cle DHT doit faire
/// 20 octets (160 bits) pour etre un node_id valide.
fn parse_key(hex_str: &str) -> Result<[u8; 20], ()> {
    hex::decode(hex_str)
        .ok()
        .and_then(|b| <[u8; 20]>::try_from(b.as_slice()).ok())
        .ok_or(())
}

/// `self.dht` du endpoint (`session.get_overlay(DHTCommunity)`).
fn require_dht(state: &AppState) -> Option<std::sync::Arc<DhtCommunity>> {
    state.session.dht()
}

/// `GET /api/ipv8/dht/statistics` — `get_statistics`.
pub async fn get_statistics(State(state): State<AppState>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_found();
    };
    let s = dht.stats_snapshot();
    let endpoints: Vec<_> = s
        .endpoints
        .iter()
        .map(|e| {
            serde_json::json!({
                "endpoint": e.endpoint,
                "node_id": e.node_id,
                "routing_table_size": e.routing_table_size,
                "routing_table_buckets": e.routing_table_buckets,
                "num_keys_in_store": e.num_keys_in_store,
            })
        })
        .collect();
    let peers_in_store: serde_json::Map<String, serde_json::Value> = s
        .num_peers_in_store
        .iter()
        .map(|(k, n)| (k.clone(), serde_json::json!(n)))
        .collect();
    let store_for_me: serde_json::Map<String, serde_json::Value> = s
        .num_store_for_me
        .iter()
        .map(|(k, n)| (k.clone(), serde_json::json!(n)))
        .collect();
    Json(serde_json::json!({
        "statistics": {
            "peer_id": s.peer_id,
            "num_tokens": s.num_tokens,
            "endpoints": endpoints,
            // `DHTDiscoveryCommunity` (notre `DhtCommunity` inclut
            // toujours `store`/`store_for_me`).
            "num_peers_in_store": peers_in_store,
            "num_store_for_me": store_for_me,
        }
    }))
    .into_response()
}

/// `GET /api/ipv8/dht/values` — `get_stored_values` : objet
/// `{hex_key: [{endpoint, public_key, key, value}]}`.
pub async fn get_stored_values(State(state): State<AppState>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_found();
    };
    let mut results = serde_json::Map::new();
    for v in dht.stored_values() {
        let entry = serde_json::json!({
            "endpoint": v.endpoint,
            "public_key": v.public_key.map(|pk| base64::engine::general_purpose::STANDARD.encode(pk)),
            "key": v.key,
            "value": hex::encode(&v.data),
        });
        if let Some(a) = results
            .entry(v.key)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
        {
            a.push(entry);
        }
    }
    Json(serde_json::Value::Object(results)).into_response()
}

/// `GET /api/ipv8/dht/values/{key}` — `get_values` (lookup +
/// compteurs `debug`).
pub async fn get_values(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_found();
    };
    let Ok(key_bytes) = parse_key(&key) else {
        return unhandled(format!("invalid hex key: {key}"));
    };
    let start = std::time::Instant::now();
    match dht.find_values_debug(&key_bytes, 0).await {
        Ok((values, debug)) => Json(serde_json::json!({
            "values": values.iter().map(|(data, public_key)| serde_json::json!({
                "public_key": public_key.as_ref().map(|pk| base64::engine::general_purpose::STANDARD.encode(pk)),
                "key": key,
                "value": hex::encode(data),
            })).collect::<Vec<_>>(),
            "debug": {
                "requests": debug.requests(),
                "responses": debug.responses,
                "responses_with_nodes": debug.responses_with_nodes,
                "responses_with_values": debug.responses_with_values,
                "time": start.elapsed().as_secs_f64(),
            }
        }))
        .into_response(),
        // `DHTError` non geree Python → 500 error_middleware.
        Err(e) => unhandled(e.to_string()),
    }
}

/// `PUT /api/ipv8/dht/values/{key}` — `put_value` : corps JSON
/// `{"value": "<hex>"}`, stocke avec `sign=True`.
pub async fn put_value(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_found();
    };
    // `await request.json()` Python : corps non-JSON → exception → 500.
    let parameters: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return unhandled(e.to_string()),
    };
    let Some(value_hex) = parameters.get("value").and_then(|v| v.as_str()) else {
        // `{"success": false, "error": "incorrect parameters"}` 400.
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "incorrect parameters"
            })),
        )
            .into_response();
    };
    let Ok(key_bytes) = parse_key(&key) else {
        return unhandled(format!("invalid hex key: {key}"));
    };
    let Ok(value) = hex::decode(value_hex) else {
        return unhandled(format!("invalid hex value: {value_hex}"));
    };
    match dht.store_value(&key_bytes, &value, true).await {
        Ok(_) => Json(serde_json::json!({ "success": true })).into_response(),
        Err(e) => unhandled(e.to_string()),
    }
}

/// `GET /api/ipv8/dht/peers/{mid}` — `get_peer` : `connect_peer`
/// puis `{"peers": [{"public_key": b64, "address": [ip, port]}]}`.
pub async fn get_peer(State(state): State<AppState>, Path(mid): Path<String>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_found();
    };
    let Ok(mid_bytes) = parse_key(&mid) else {
        return unhandled(format!("invalid hex mid: {mid}"));
    };
    match dht.connect_peer(&mid_bytes, None).await {
        Ok(nodes) => Json(serde_json::json!({
            "peers": nodes.iter().map(|n| {
                let addr = n.address();
                serde_json::json!({
                    "public_key": base64::engine::general_purpose::STANDARD.encode(&n.key),
                    // `node.address` Python = tuple ("ip", port).
                    "address": addr.to_socket_addr()
                        .map(|sa| serde_json::json!([sa.ip().to_string(), sa.port()]))
                        .unwrap_or(serde_json::Value::Null),
                })
            }).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => unhandled(e.to_string()),
    }
}

/// `GET /api/ipv8/dht/buckets` — `get_buckets` (200 `{"buckets": []}`
/// quand le DHT est absent, contrairement aux autres routes).
pub async fn get_buckets(State(state): State<AppState>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return Json(serde_json::json!({ "buckets": [] })).into_response();
    };
    let buckets: Vec<_> = dht
        .buckets_snapshot()
        .into_iter()
        .map(|b| {
            serde_json::json!({
                "prefix": b.prefix,
                "last_changed": b.last_changed,
                "endpoint": b.endpoint,
                "peers": b.peers.iter().map(|p| serde_json::json!({
                    "ip": p.ip,
                    "port": p.port,
                    "mid": p.mid,
                    "id": p.id,
                    "failed": p.failed,
                    "last_contact": p.last_contact,
                    // `int(distance())` Python tient sur 160 bits : on
                    // rend la decimale exacte en chaine (JSON Number
                    // est limite a 128 bits sans arbitrary_precision).
                    "distance": p.distance,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(serde_json::json!({ "buckets": buckets })).into_response()
}

/// `GET /api/ipv8/dht/buckets/{prefix}/refresh` — `refresh_bucket`.
pub async fn refresh_bucket(State(state): State<AppState>, Path(prefix): Path<String>) -> Response {
    let Some(dht) = require_dht(&state) else {
        return dht_not_loaded();
    };
    match dht.refresh_bucket(&prefix).await {
        Ok(()) => Json(serde_json::json!({ "success": true })).into_response(),
        Err(DhtError::NoSuchBucket) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "no such bucket"
            })),
        )
            .into_response(),
        // `error and not success` Python → `{"success": false,
        // "error": str(e)}` en **200**.
        Err(e) => Json(serde_json::json!({
            "success": false,
            "error": e.to_string()
        }))
        .into_response(),
    }
}
