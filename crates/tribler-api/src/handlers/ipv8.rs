//! Handlers `/api/ipv8` — equivalent de `tribler.core.restapi.
//! ipv8_endpoint` (pyipv8 `RootEndpoint` : `overlays`, `tunnel`,
//! `network`, `isolation`, `noblockdht`, `overlays/statistics`).
//!
//! Formes de reponse fideles a pyipv8 : erreurs anticipees en
//! `{"success": false, "error": ...}` ; exceptions non gerees en
//! 500 `{"error": {"handled": false, "message"}}`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use base64::Engine;
use tribler_ipv8::overlays::{addr_parts, OverlayInfo};
use tribler_ipv8::{AggregateStats, NetworkStat, UdpAddress};

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
    endpoint: &tribler_ipv8::UdpEndpoint,
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
    endpoint: &tribler_ipv8::UdpEndpoint,
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
) -> Option<std::sync::Arc<tribler_tunnel::community::TunnelCommunity>> {
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

/// `GET /api/ipv8/tunnel/peers` — pairs tunnel connus + flags.
pub async fn get_tunnel_peers(State(state): State<AppState>) -> Response {
    let Some(tunnel) = tunnel_of(&state) else {
        return Json(serde_json::json!({ "peers": [] })).into_response();
    };
    Json(serde_json::json!({ "peers": tunnel.tunnel_peers_info() })).into_response()
}
