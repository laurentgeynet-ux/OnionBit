//! Handlers `/api/libtorrent` — equivalent de
//! `tribler.core.libtorrent.restapi.libtorrent_endpoint`.
//!
//! Le moteur est `librqbit` (pas libtorrent) : les reglages exposes
//! sont le sous-ensemble pertinent de `EngineConfig`, avec les noms de
//! cles `lt::settings_pack` quand un equivalent existe.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/libtorrent/settings?hop={hops}` — reglages d'une
/// session moteur (`get_libtorrent_settings` Python : dict plat de
/// `lt::settings_pack` ; on rend les equivalents rqbit).
/// Accepte `hop` (parite Python) et `session` (retro-compatibilite).
#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    /// Saut : `0` = principale, `1..=3` = lane anonyme.
    pub hop: Option<String>,
    /// Alias conserve pour compatibilite.
    pub session: Option<String>,
}

impl SessionQuery {
    pub fn hops(&self) -> usize {
        self.hop
            .as_deref()
            .or(self.session.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

pub async fn get_libtorrent_settings(
    State(state): State<AppState>,
    Query(q): Query<SessionQuery>,
) -> Json<serde_json::Value> {
    let hops = q.hops();
    let cfg = state.session.config();
    let engine_cfg = if hops == 0 {
        cfg.engine.clone()
    } else {
        // Lane anonyme : config deduite (proxy SOCKS5 + uTP-only).
        let mut c = cfg.engine.clone();
        if let Some(stack) = state.session.ipv8() {
            if let Some((_, addr)) = stack.anon_lanes().iter().find(|(h, _)| *h == hops) {
                c.socks5_proxy = Some(format!("socks5://{addr}"));
                c.utp_only = true;
            }
        }
        c
    };
    Json(serde_json::json!({
        "hop": hops,
        "settings": {
            "listen_interfaces": engine_cfg
                .listen_addr_v6
                .map(|a| a.to_string())
                .or_else(|| {
                    engine_cfg
                        .listen_port
                        .map(|p| format!("{}:{p}", engine_cfg.listen_ip))
                })
                .unwrap_or_default(),
            "proxy_type": engine_cfg.socks5_proxy.as_ref().map(|_| 5),
            "proxy_hostname": engine_cfg.socks5_proxy,
            "enable_dht": engine_cfg.enable_dht,
            "enable_lsd": !engine_cfg.disable_lsd,
            "enable_upnp": engine_cfg.enable_upnp,
            "enable_utp": engine_cfg.enable_utp,
            "enable_natpmp": false,
            "utp_only": engine_cfg.utp_only,
            "peer_connections_limit": engine_cfg.peer_limit.map(|v| v as i64).unwrap_or(-1),
            "user_agent": onionbit_bittorrent::config::CLIENT_NAME,
            "download_rate_limit": 0,
            "upload_rate_limit": 0,
        }
    }))
}

/// `GET /api/libtorrent/session?hop={hops}` — etat d'une session
/// (`get_libtorrent_session_info` : flags `hopX_enabled` + stats).
/// Accepte `hop` (parite Python) et `session` (retro-compatibilite).
pub async fn get_libtorrent_session_info(
    State(state): State<AppState>,
    Query(q): Query<SessionQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hops = q.hops();
    let engine = if hops == 0 {
        state.session.engine().clone()
    } else {
        let stack = state
            .session
            .ipv8()
            .ok_or_else(|| ApiError::not_found("hop session does not exist"))?;
        stack
            .anon_lanes()
            .iter()
            .find(|(h, _)| *h == hops)
            .ok_or_else(|| ApiError::not_found("hop session does not exist"))?;
        stack
            .anon_engine(hops)
            .await
            .map_err(|e| ApiError::not_found(format!("hop session does not exist: {e}")))?
    };
    let downloads = engine.list();
    Ok(Json(serde_json::json!({
        "hop": hops,
        "session": {
            "torrents": downloads.len(),
            "active_downloads": downloads.iter()
                .filter(|d| d.state == onionbit_bittorrent::DownloadState::Downloading)
                .count(),
            "listen_port": engine.listen_addr().map(|a| a.port()),
        }
    })))
}
