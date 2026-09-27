//! Handlers `/api/ipv8` — equivalent de `tribler.core.restapi.
//! ipv8_endpoint` (pyipv8 `RootEndpoint` : `overlays`, `tunnel`).

use axum::extract::State;
use axum::Json;

use crate::error::ApiError;
use crate::state::AppState;

/// `503`-equivalent Python quand IPv8 est desactive : l'endpoint
/// repond une erreur geree.
fn require_stack(state: &AppState) -> Result<std::sync::Arc<tribler_core::Ipv8Stack>, ApiError> {
    state
        .session
        .ipv8()
        .ok_or_else(|| ApiError::bad_request("ipv8 stack inactive"))
}

/// `GET /api/ipv8/overlays` — liste des communities actives
/// (`get_overlays` pyipv8 : nom + prefixe hex + pairs).
pub async fn get_overlays(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let mut overlays = vec![
        serde_json::json!({
            "name": "DiscoveryCommunity",
            "id": hex::encode(tribler_ipv8::discovery::DISCOVERY_COMMUNITY_ID),
            "peers": stack.discovery.peer_count(),
        }),
        serde_json::json!({
            "name": "ContentDiscoveryCommunity",
            "id": hex::encode(
                tribler_ipv8::content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID
            ),
            "peers": stack
                .network
                .peers_for_service(
                    &tribler_ipv8::content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID,
                )
                .len(),
        }),
    ];
    if stack.tunnel.is_some() {
        let count = stack
            .network
            .peers_for_service(&tribler_tunnel::TUNNEL_COMMUNITY_ID)
            .len()
            + stack
                .network
                .peers_for_service(&tribler_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID)
                .len();
        overlays.push(serde_json::json!({
            "name": "TunnelCommunity",
            "id": hex::encode(tribler_tunnel::TUNNEL_COMMUNITY_ID),
            "peers": count,
        }));
    }
    Ok(Json(serde_json::json!({ "overlays": overlays })))
}

/// `GET /api/ipv8/tunnel/settings` — reglages tunnel
/// (`get_settings` pyipv8 : `min_circuits`, `peer_flags`...).
pub async fn get_tunnel_settings(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(serde_json::json!({
        "settings": {
            "peer_flags": state.session.config().ipv8.peer_flags,
            "circuits": tunnel.circuit_count(),
            "community_id": hex::encode(
                if state.session.config().ipv8.tribler_tunnel_community {
                    tribler_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID
                } else {
                    tribler_tunnel::TUNNEL_COMMUNITY_ID
                }
            ),
        }
    })))
}

/// `GET /api/ipv8/tunnel/circuits` — circuits connus.
pub async fn get_tunnel_circuits(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(
        serde_json::json!({ "circuits": tunnel.circuits_info() }),
    ))
}

/// `GET /api/ipv8/tunnel/relays` — relais actifs.
pub async fn get_tunnel_relays(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(serde_json::json!({ "relays": tunnel.relays_info() })))
}

/// `GET /api/ipv8/tunnel/exits` — sockets de sortie actives.
pub async fn get_tunnel_exits(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(serde_json::json!({ "exits": tunnel.exits_info() })))
}

/// `GET /api/ipv8/tunnel/swarms` — swarms hidden services connus.
pub async fn get_tunnel_swarms(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(serde_json::json!({ "swarms": tunnel.swarms_info() })))
}

/// `GET /api/ipv8/tunnel/peers` — pairs tunnel connus + flags.
pub async fn get_tunnel_peers(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stack = require_stack(&state)?;
    let tunnel = stack
        .tunnel
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("tunnel community inactive"))?;
    Ok(Json(
        serde_json::json!({ "peers": tunnel.tunnel_peers_info() }),
    ))
}
