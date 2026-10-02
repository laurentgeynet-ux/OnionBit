// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/ipv8` — equivalent de `tribler.core.restapi.
//! ipv8_endpoint` (pyipv8 `RootEndpoint` : `overlays`, `tunnel`,
//! `network`, `isolation`, `noblockdht`, `overlays/statistics`).
//!
//! Formes de reponse fideles a pyipv8 : erreurs anticipees en
//! `{"success": false, "error": ...}` ; exceptions non gerees en
//! 500 `{"error": {"handled": false, "message"}}`.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;

use base64::Engine;
use onionbit_ipv8::overlays::{addr_parts, OverlayInfo};
use onionbit_ipv8::{AggregateStats, NetworkStat, UdpAddress};

use crate::state::AppState;

/// Exception non geree Python → `error_middleware` :
/// 500 `{"error": {"handled": false, "message"}}`.
fn unhandled(message: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": { "handled": false, "message": message }
        })),
    )
        .into_response()
}

/// `{"success": false, "error": ...}` avec statut (`Response` brute
/// pyipv8 — sans l'enveloppe `handled`).
fn plain_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "success": false,
            "error": message
        })),
    )
        .into_response()
}

/// `NetworkStat.to_dict()` pyipv8.
fn stat_json(s: &NetworkStat) -> serde_json::Value {
    serde_json::json!({
        "identifier": s.identifier,
        "num_up": s.num_up,
        "num_down": s.num_down,
        "bytes_up": s.bytes_up,
        "bytes_down": s.bytes_down,
        "first_measured_up": s.first_measured_up,
        "first_measured_down": s.first_measured_down,
        "last_measured_up": s.last_measured_up,
        "last_measured_down": s.last_measured_down,
    })
}

/// `get_aggregate_statistics(prefix)` → dict
/// `{num_up,num_down,bytes_up,bytes_down,diff_time}`.
fn aggregate_json(agg: &AggregateStats) -> serde_json::Value {
    serde_json::json!({
        "num_up": agg.num_up,
        "num_down": agg.num_down,
        "bytes_up": agg.bytes_up,
        "bytes_down": agg.bytes_down,
        "diff_time": agg.diff_time,
    })
}

/// Element de `{"overlays": [...]}` (`OverlaySchema` pyipv8) :
/// `statistics` = agregat de l'endpoint (zeros si non suivi).
async fn overlay_json(
    info: &OverlayInfo,
    endpoint: &onionbit_ipv8::UdpEndpoint,
) -> serde_json::Value {
    let stats = endpoint.get_aggregate_statistics(&info.prefix()).await;
    let (wan_ip, wan_port) = addr_parts(Some(&info.my_estimated_wan));
    let (lan_ip, lan_port) = addr_parts(Some(&info.my_estimated_lan));
    serde_json::json!({
        "id": hex::encode(info.community_id),
        "my_peer": info.my_peer_hex,
        "global_time": info.global_time,
        "peers": info.peers.iter().map(|p| serde_json::json!({
            "ip": p.ip,
            "port": p.port,
            "public_key": p.public_key_hex,
        })).collect::<Vec<_>>(),
        "overlay_name": info.overlay_name,
        "statistics": aggregate_json(&stats),
        "max_peers": info.max_peers,
        "is_isolated": info.is_isolated,
        "my_estimated_wan": { "ip": wan_ip, "port": wan_port },
        "my_estimated_lan": { "ip": lan_ip, "port": lan_port },
        "strategies": info.strategies.iter().map(|s| serde_json::json!({
            "name": s.name,
            "target_peers": s.target_peers,
        })).collect::<Vec<_>>(),
    })
}

/// `statistics_by_name` : `{msg_id}:{handler|unknown}` →
/// `NetworkStat.to_dict()`.
async fn statistics_by_name(
    info: &OverlayInfo,
    endpoint: &onionbit_ipv8::UdpEndpoint,
) -> serde_json::Map<String, serde_json::Value> {
    let mut named = serde_json::Map::new();
    for (msg_id, stat) in endpoint.get_statistics(&info.prefix()).await {
        let name = info.msg_name(msg_id).unwrap_or("unknown");
        named.insert(format!("{msg_id}:{name}"), stat_json(&stat));
    }
    named
}

/// `GET /api/ipv8/overlays` — `get_overlays` pyipv8 :
/// `{"overlays": [OverlaySchema]}` (`[]` si session IPv8 absente).
pub async fn get_overlays(State(state): State<AppState>) -> Response {
    let Some(stack) = state.session.ipv8() else {
        // `self.session is None` Python → `{"overlays": []}`.
        return Json(serde_json::json!({ "overlays": [] })).into_response();
    };
    let mut overlays = Vec::new();
    for info in stack.overlays_info() {
        overlays.push(overlay_json(&info, &stack.endpoint).await);
    }
    Json(serde_json::json!({ "overlays": overlays })).into_response()
}

/// `GET /api/ipv8/overlays/statistics` — `get_statistics` pyipv8 :
/// `{"statistics": [{OverlayName: {id:handler: NetworkStat}}]}`.
/// Notre `UdpEndpoint` est toujours un `StatisticsEndpoint` —
/// la condition `statistics_supported` est donc toujours vraie.
pub async fn get_overlay_statistics(State(state): State<AppState>) -> Response {
    let Some(stack) = state.session.ipv8() else {
        return Json(serde_json::json!({ "statistics": [] })).into_response();
    };
    let mut statistics = Vec::new();
    for info in stack.overlays_info() {
        let named = statistics_by_name(&info, &stack.endpoint).await;
        statistics.push(serde_json::json!({ info.overlay_name: named }));
    }
    Json(serde_json::json!({ "statistics": statistics })).into_response()
}

/// `POST /api/ipv8/overlays/statistics` — `enable_statistics` pyipv8.
/// Corps : `{"enable": bool, "all"?: bool, "overlay_name"?: str}`.
pub async fn post_overlay_statistics(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(stack) = state.session.ipv8() else {
        // `self.session is None` → 412 "IPv8 is not running".
        return (
            StatusCode::PRECONDITION_FAILED,
            Json(serde_json::json!({
                "success": false,
                "error": "IPv8 is not running"
            })),
        )
            .into_response();
    };
    // `await request.json()` Python : corps non-JSON → exception → 500.
    let args: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return unhandled(e.to_string()),
    };
    let Some(enable) = args.get("enable").and_then(|v| v.as_bool()) else {
        return plain_error(StatusCode::BAD_REQUEST, "Parameter \"enable\" is required");
    };
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let overlay_name = args.get("overlay_name").and_then(|v| v.as_str());
    if !all && overlay_name.is_none() {
        return (
            StatusCode::PRECONDITION_FAILED,
            Json(serde_json::json!({
                "success": false,
                "error": "Parameter \"all\" or \"overlay_name\" is required"
            })),
        )
            .into_response();
    }
    stack
        .enable_overlay_statistics(enable, overlay_name, all)
        .await;
    Json(serde_json::json!({ "success": true })).into_response()
}

/// `GET /api/ipv8/network` — `retrieve_peers` pyipv8 :
/// `{"peers": {b64(mid): {ip, port, public_key(b64), services[b64]}}}`
/// (`{}` si session IPv8 absente).
pub async fn get_network(State(state): State<AppState>) -> Response {
    use base64::engine::general_purpose::STANDARD as B64;
    let Some(stack) = state.session.ipv8() else {
        return Json(serde_json::json!({ "peers": {} })).into_response();
    };
    let mut peers = serde_json::Map::new();
    for peer in stack.network.verified_peers() {
        let (ip, port) = addr_parts(peer.address.as_ref());
        let services: Vec<_> = stack
            .network
            .services_for_peer(&peer.public_key_bin)
            .iter()
            .map(|s| serde_json::Value::String(B64.encode(s)))
            .collect();
        peers.insert(
            B64.encode(peer.mid),
            serde_json::json!({
                "ip": ip,
                "port": port,
                "public_key": B64.encode(&peer.public_key_bin),
                "services": services,
            }),
        );
    }
    Json(serde_json::json!({ "peers": peers })).into_response()
}

/// `POST /api/ipv8/isolation` — `handle_post` pyipv8 : ajoute une
/// adresse a un service (`exitnode` → walk tunnel, `bootstrapnode` →
/// blacklist + walk + bootstrapper).
pub async fn post_isolation(State(state): State<AppState>, body: Bytes) -> Response {
    // `await request.json()` Python : corps non-JSON → exception → 500.
    let args: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return unhandled(e.to_string()),
    };
    if !args.is_object() || args.get("ip").is_none() || args.get("port").is_none() {
        return plain_error(
            StatusCode::BAD_REQUEST,
            "Parameters \"ip\" and \"port\" are required",
        );
    }
    if args.get("exitnode").is_none() && args.get("bootstrapnode").is_none() {
        return plain_error(
            StatusCode::BAD_REQUEST,
            "Parameter \"exitnode\" or \"bootstrapnode\" is required",
        );
    }
    let Some(stack) = state.session.ipv8() else {
        // `self.session.overlays` sur `None` → AttributeError Python
        // → 500 `error_middleware`.
        return unhandled("'NoneType' object has no attribute 'network'".into());
    };
    let ip = args["ip"].as_str().unwrap_or_default();
    let port = args["port"].as_u64().unwrap_or(0) as u16;
    // `(ip, port)` Python : numerique -> UDPv4/6Address, sinon
    // `DomainAddress` (resolu par l'endpoint a l'envoi).
    let addr = match ip.parse::<std::net::Ipv4Addr>() {
        Ok(v4) => UdpAddress::Ipv4(std::net::SocketAddrV4::new(v4, port)),
        Err(_) => match ip.parse::<std::net::Ipv6Addr>() {
            Ok(v6) => UdpAddress::Ipv6(std::net::SocketAddrV6::new(v6, port, 0, 0)),
            Err(_) => UdpAddress::Domain(ip.to_string(), port),
        },
    };
    // `exitnode` est prioritaire quand les deux sont presents
    // (`if/else` Python).
    if args.get("exitnode").is_some() {
        stack.walk_exit_nodes(&addr).await;
    } else {
        stack.add_bootstrap_node(&addr).await;
    }
    Json(serde_json::json!({ "success": true })).into_response()
}

/// `GET /api/ipv8/noblockdht/{mid}` — `handle_get` de
/// `NoBlockDHTEndpoint` : `connect_peer` en tache, retour immediat.
pub async fn get_noblock_dht(State(state): State<AppState>, Path(mid): Path<String>) -> Response {
    let Some(stack) = state.session.ipv8() else {
        // `not self.dht` → 404 `{"error": ...}` (sans "success").
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "DHT community not found" })),
        )
            .into_response();
    };
    // `unhexlify` Python : hex invalide → exception → 500. La
    // longueur n'est pas controlee : `connect_peer` tourne en tache
    // et ses erreurs sont loggees, pas renvoyees — un mid hex valide
    // d'une autre taille repond donc `success` sans connect_peer.
    let Ok(bytes) = hex::decode(&mid) else {
        return unhandled(format!("invalid hex mid: {mid}"));
    };
    if let Ok(mid_bytes) = <[u8; 20]>::try_from(bytes.as_slice()) {
        if !stack.connect_peer_noblock(mid_bytes) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "DHT community not found" })),
            )
                .into_response();
        }
    }
    Json(serde_json::json!({ "success": true })).into_response()
}

/// `self.tunnels` du endpoint : `TunnelCommunity` de la stack, ou
/// `None` → les handlers Python repondent une collection vide en 200
/// (`{"circuits": []}`, `{"peers": []}`, ...) — sauf circuits de
/// speed-test/dht, qui repondent 404 (etape 27).
fn tunnel_of(
    state: &AppState,
) -> Option<std::sync::Arc<onionbit_tunnel::community::TunnelCommunity>> {
    state.session.ipv8().and_then(|s| s.tunnel.clone())
}

/// `GET /api/ipv8/tunnel/settings` — reglages tunnel
/// (`get_settings` pyipv8 : `{"settings": {}}` si `tunnels is None`).
pub async fn get_tunnel_settings(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "settings": {} })).into_response();
    };
    Json(serde_json::json!({
        "settings": {
            "peer_flags": state.session.config().ipv8.peer_flags,
            "circuits": tunnel.circuit_count(),
            "community_id": hex::encode(tunnel.community_id()),
        }
    }))
    .into_response()
}

/// `GET /api/ipv8/tunnel/circuits` — circuits connus
/// (`{"circuits": []}` quand `tunnels is None`).
pub async fn get_tunnel_circuits(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "circuits": [] })).into_response();
    };
    Json(serde_json::json!({ "circuits": tunnel.circuits_info() })).into_response()
}

/// `GET /api/ipv8/tunnel/relays` — relais actifs.
pub async fn get_tunnel_relays(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "relays": [] })).into_response();
    };
    Json(serde_json::json!({ "relays": tunnel.relays_info() })).into_response()
}

/// `GET /api/ipv8/tunnel/exits` — sockets de sortie actives.
pub async fn get_tunnel_exits(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "exits": [] })).into_response();
    };
    Json(serde_json::json!({ "exits": tunnel.exits_info() })).into_response()
}

/// `GET /api/ipv8/tunnel/swarms` — swarms hidden services connus.
pub async fn get_tunnel_swarms(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "swarms": [] })).into_response();
    };
    Json(serde_json::json!({ "swarms": tunnel.swarms_info() })).into_response()
}

/// `GET /api/ipv8/tunnel/guards` — guard nodes actifs/reserve
/// (ADR-0010, extension Rust sans equivalent pyipv8) :
/// `{"guards": [], "enabled": bool}` — `enabled` reflete la config,
/// la liste est vide tant qu'aucun guard n'est adopte ou si la
/// feature est desactivee.
pub async fn get_tunnel_guards(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "guards": [], "enabled": false })).into_response();
    };
    Json(serde_json::json!({
        "guards": tunnel.guards.guards_info(),
        "enabled": tunnel.guards.is_enabled(),
    }))
    .into_response()
}

/// `GET /api/ipv8/tunnel/debug/circuit-downloads` — correlation
/// circuits <-> downloads anonymes. **Extension Rust** sans
/// equivalent pyipv8 (Python garde `download_states` interne) : pour
/// chaque swarm cache lie a un download, l'info-hash reel,
/// l'info-hash de lookup, la lane `hops`, l'etat du download et les
/// circuits du tunnel dont `info_hash` correspond — plus
/// `swarm_peers` (connexions e2e du swarm rejoint) et `seeder`.
/// `{"downloads": []}` sans tunnel.
pub async fn get_tunnel_circuit_downloads(State(state): State<AppState>) -> Response {
    let Some(stack) = state.session.ipv8() else {
        return Json(serde_json::json!({ "downloads": [] })).into_response();
    };
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "downloads": [] })).into_response();
    };
    let circuits = tunnel.circuits_info();
    let swarms = tunnel.swarms_info();
    let downloads: Vec<serde_json::Value> = stack
        .swarm_downloads()
        .into_iter()
        .map(|d| {
            let lookup_hex = hex::encode(d.lookup_info_hash);
            let bound: Vec<&onionbit_tunnel::community::CircuitInfo> = circuits
                .iter()
                .filter(|c| c.info_hash.as_deref() == Some(lookup_hex.as_str()))
                .collect();
            let swarm = swarms.iter().find(|s| s.info_hash == lookup_hex);
            serde_json::json!({
                "info_hash": hex::encode(d.info_hash),
                "lookup_info_hash": lookup_hex,
                "hops": d.hops,
                "state": format!("{:?}", d.state).to_uppercase(),
                "seeder": swarm.map(|s| s.seeder).unwrap_or(false),
                "swarm_peers": swarm.map(|s| s.num_connections).unwrap_or(0),
                "circuits": bound,
            })
        })
        .collect();
    Json(serde_json::json!({ "downloads": downloads })).into_response()
}

/// `GET /api/ipv8/tunnel/peers` — pairs tunnel connus + flags.
pub async fn get_tunnel_peers(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "peers": [] })).into_response();
    };
    Json(serde_json::json!({ "peers": tunnel.tunnel_peers_info() })).into_response()
}

/// Corps `{"error": ...}` brut de `Response(dict, status)` pyipv8
/// (forme propre au `tunnel_endpoint` — sans `success` ni
/// `handled`).
fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// `IntroductionPoint.to_dict()` pyipv8.
fn intro_point_json(ip: &onionbit_tunnel::routing::IntroductionPoint) -> serde_json::Value {
    let (ip_s, port) = addr_parts(Some(&ip.address));
    serde_json::json!({
        "address": {
            "ip": ip_s,
            "port": port,
            "public_key": hex::encode(&ip.peer_key),
        },
        "seeder_pk": hex::encode(&ip.seeder_pk),
        "source": ip.source,
    })
}

/// `DHTIntroPointPayload` → `IntroductionPoint` : delegation a
/// `onionbit_tunnel::hidden_services::unpack_dht_intro_point`
/// (implementation unique, partagee avec `swarm_lookup`).
fn unpack_dht_intro_point(data: &[u8]) -> Option<onionbit_tunnel::routing::IntroductionPoint> {
    onionbit_tunnel::hidden_services::unpack_dht_intro_point(data)
}

/// `GET /api/ipv8/tunnel/swarms/{infohash}/size` — `get_swarm_size`
/// pyipv8 : `{"swarm_size": n}` (`{"swarms": []}` sans tunnel).
pub async fn get_swarm_size(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        // `tunnels is None` → `Response({"swarms": []})`.
        return Json(serde_json::json!({ "swarms": [] })).into_response();
    };
    // `unhexlify` : hex invalide → `binascii.Error` → 500.
    let Ok(raw) = hex::decode(&infohash) else {
        return unhandled("Non-hexadecimal digit found".into());
    };
    // `request.query.get("hops", 1)` : present → CHAINE →
    // `select_circuit(hops="n")` ne match aucun circuit → toutes les
    // requetes levent `RuntimeError` → `swarm_size` 0 (quirk Python).
    if query.contains_key("hops") {
        return Json(serde_json::json!({ "swarm_size": 0 })).into_response();
    }
    // `PeersRequestPayload` `"20s"` : `struct.pack` tronque/pad —
    // meme comportement pour un infohash d'une autre longueur.
    let mut ih = [0u8; 20];
    let n = raw.len().min(20);
    ih[..n].copy_from_slice(&raw[..n]);
    let swarm_size = tunnel.estimate_swarm_size(ih, 1).await;
    Json(serde_json::json!({ "swarm_size": swarm_size })).into_response()
}

/// `GET /api/ipv8/tunnel/peers/dht` — `get_dht_peers` pyipv8 : points
/// d'introduction stockes dans la DHT locale (`[]` brut sans tunnel
/// ni provider DHT).
pub async fn get_dht_peers(State(state): State<AppState>) -> Response {
    let Some(stack) = state.session.ipv8() else {
        return Json(serde_json::json!([])).into_response();
    };
    let (Some(_tunnel), Some(dht)) = (stack.tunnel.as_ref(), stack.dht.as_ref()) else {
        return Json(serde_json::json!([])).into_response();
    };
    // `ips_by_infohash[key] = []` pour chaque cle de chaque storage ;
    // les valeurs non-`DHTIntroPointPayload` sont ignorees
    // (`PackError` → `continue`).
    let mut grouped: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for sv in dht.stored_values() {
        let pos = *index.entry(sv.key.clone()).or_insert_with(|| {
            grouped.push((sv.key, Vec::new()));
            grouped.len() - 1
        });
        if let Some(ip) = unpack_dht_intro_point(&sv.data) {
            grouped[pos].1.push(intro_point_json(&ip));
        }
    }
    Json(serde_json::Value::Array(
        grouped
            .into_iter()
            .map(|(key, peers)| serde_json::json!({ "info_hash": key, "peers": peers }))
            .collect(),
    ))
    .into_response()
}

/// `GET /api/ipv8/tunnel/peers/pex` — `get_pex_peers` pyipv8 : points
/// d'introduction du store PEX par swarm (`[]` brut sans tunnel).
pub async fn get_pex_peers(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!([])).into_response();
    };
    let rows = tunnel.pex_intro_points();
    Json(serde_json::Value::Array(
        rows.into_iter()
            .map(|(ih, ips)| {
                serde_json::json!({
                    "info_hash": hex::encode(ih),
                    "peers": ips.iter().map(intro_point_json).collect::<Vec<_>>(),
                })
            })
            .collect(),
    ))
    .into_response()
}

/// `isdigit()` Python : non vide et chiffres ASCII uniquement.
fn is_digit_str(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `run_speed_test` pyipv8 : valide `request_size`/`response_size`/
/// `test_time_ms`, lance `run_speedtest` et streame les snapshots en
/// `text/event-stream` (`speed: {...}\n` par ligne).
async fn run_speed_test(
    tunnel: std::sync::Arc<onionbit_tunnel::community::TunnelCommunity>,
    circuit_id: u32,
    params: &std::collections::HashMap<String, String>,
) -> Response {
    // `params.get("request_size", 50)` : present → CHAINE →
    // `0 <= "50"` leve TypeError → 500 (quirk Python conserve ;
    // le schema `Integer` n'est pas applique sans
    // `validation_middleware`).
    if params.contains_key("request_size") || params.contains_key("response_size") {
        return unhandled("'<' not supported between instances of 'int' and 'str'".into());
    }
    /// `request_size`/`response_size` par defaut Python.
    const DEFAULT_REQUEST_SIZE: u16 = 50;
    const DEFAULT_RESPONSE_SIZE: u16 = 1024;
    let test_time_ms = params.get("test_time_ms").map_or("5000", String::as_str);
    let parsed = is_digit_str(test_time_ms)
        .then(|| test_time_ms.parse::<u64>().ok())
        .flatten()
        .unwrap_or(0);
    if !(1..=60_000).contains(&parsed) {
        return error_response(StatusCode::BAD_REQUEST, "invalid test time specified");
    }
    let rx = tunnel.run_speedtest(
        circuit_id,
        parsed,
        DEFAULT_REQUEST_SIZE,
        DEFAULT_RESPONSE_SIZE,
    );
    // `callback` pyipv8 : `{tid: [ts_envoi, octets_envoyes,
    // ts_reception, octets_recus]}` — `up`/`down` en MiB/s sur les
    // nouveaux echantillons, ligne `speed: {json}` par snapshot.
    let mut tx_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut rx_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let stream =
        tokio_stream::wrappers::ReceiverStream::new(rx).filter_map(move |(stats, _done)| {
            let mut send_times: Vec<(u32, u64, u64)> = stats
                .iter()
                .filter(|(tid, _)| !tx_ids.contains(*tid))
                .map(|(tid, s)| (*tid, s[2], s[3]))
                .collect();
            send_times.sort_by_key(|e| e.1);
            let mut recv_times: Vec<(u32, u64, u64)> = stats
                .iter()
                .filter(|(tid, s)| !rx_ids.contains(*tid) && s[2] != 0)
                .map(|(tid, s)| (*tid, s[2], s[3]))
                .collect();
            recv_times.sort_by_key(|e| e.1);
            tx_ids.extend(send_times.iter().map(|e| e.0));
            rx_ids.extend(recv_times.iter().map(|e| e.0));
            // `send_times[-1]` leve IndexError sur snapshot vide →
            // le callback Python n'ecrit rien ce tour-ci.
            let Some((_, first_send_ts, _)) = send_times.first().copied() else {
                return futures_util::future::ready(None);
            };
            let last_send_ts = send_times.last().map(|e| e.1).unwrap_or(0);
            let up_time = last_send_ts.saturating_sub(first_send_ts) as f64 / 1000.0;
            let sent_bytes: u64 = send_times.iter().map(|e| e.2).sum();
            let up = if up_time > 0.0 {
                sent_bytes as f64 / up_time
            } else {
                0.0
            };
            let down_time = match (recv_times.first(), recv_times.last()) {
                (Some(f), Some(l)) => l.1.saturating_sub(f.1) as f64 / 1000.0,
                _ => 0.0,
            };
            let recv_bytes: u64 = recv_times.iter().map(|e| e.2).sum();
            let down = if down_time > 0.0 {
                recv_bytes as f64 / down_time
            } else {
                0.0
            };
            let line = format!(
                "speed: {}\n",
                serde_json::json!({
                    "up": up / 1_048_576.0,
                    "down": down / 1_048_576.0,
                })
            );
            futures_util::future::ready(Some(Ok::<Bytes, std::convert::Infallible>(Bytes::from(
                line,
            ))))
        });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

/// `GET /api/ipv8/tunnel/circuits/test` — `speed_test_new_circuit` :
/// circuit `SPEED_TEST` temporaire, test, puis destruction.
pub async fn speed_test_new_circuit(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // `self.tunnels is None` verifie AVANT `goal_hops` (404 prioritaire).
    let Some(tunnel) = tunnel_of(&state) else {
        return error_response(StatusCode::NOT_FOUND, "TunnelCommunity is not initialized");
    };
    let goal_hops = query.get("goal_hops").map_or("1", String::as_str);
    let hops = is_digit_str(goal_hops)
        .then(|| goal_hops.parse::<usize>().ok())
        .flatten()
        .unwrap_or(0);
    if !(1..=3).contains(&hops) {
        return error_response(StatusCode::BAD_REQUEST, "invalid number of hops specified");
    }
    let Some(circuit_id) = tunnel
        .create_circuit_with_flags(
            hops,
            onionbit_tunnel::routing::CIRCUIT_TYPE_SPEED_TEST,
            &[onionbit_tunnel::routing::PEER_FLAG_SPEED_TEST],
        )
        .await
        .ok()
        .flatten()
    else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create circuit",
        );
    };
    // `await circuit.ready` : le circuit doit etre pret.
    if !tunnel.await_circuit_ready(circuit_id).await {
        tunnel
            .remove_circuit(circuit_id, "speed test finished")
            .await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create circuit",
        );
    }
    let result = run_speed_test(tunnel.clone(), circuit_id, &query).await;
    tunnel
        .remove_circuit(circuit_id, "speed test finished")
        .await;
    result
}

/// `GET /api/ipv8/tunnel/circuits/{circuit_id}/test` —
/// `speed_test_existing_circuit` : circuit `READY` avec flag
/// `PEER_FLAG_SPEED_TEST`.
pub async fn speed_test_existing_circuit(
    State(state): State<AppState>,
    Path(circuit_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // `circuit_id.isdigit()` avant `self.tunnels is None`.
    if !is_digit_str(&circuit_id) {
        return error_response(StatusCode::BAD_REQUEST, "circuit_id must be an integer");
    }
    let Some(tunnel) = tunnel_of(&state) else {
        return error_response(StatusCode::NOT_FOUND, "TunnelCommunity is not initialized");
    };
    // `int(circuit_id)` Python : precision arbitraire — un entier
    // > u32 ne matche jamais un circuit → 404.
    let cid = circuit_id.parse::<u64>().unwrap_or(u64::MAX);
    let Some(info) = tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id as u64 == cid)
    else {
        return error_response(StatusCode::NOT_FOUND, "could not find requested circuit");
    };
    if info.state != onionbit_tunnel::routing::CIRCUIT_STATE_READY {
        return error_response(
            StatusCode::BAD_REQUEST,
            "the requested circuit is not ready to transfer data",
        );
    }
    if info.ctype == onionbit_tunnel::routing::CIRCUIT_TYPE_DATA
        && info.exit_flags & onionbit_tunnel::routing::PEER_FLAG_SPEED_TEST == 0
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "the requested circuit does not support speed testing",
        );
    }
    run_speed_test(tunnel, info.circuit_id, &query).await
}
