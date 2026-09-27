//! Handlers `/api/settings` — equivalent de
//! `tribler.core.restapi.settings_endpoint`.

use axum::extract::State;
use axum::Json;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/settings` — configuration effective de la session, groupee
/// comme le dictionnaire de config Python (`ipv8`, `libtorrent`,
/// `watch_folder`, `rss`, `torrent_checker`, `download_defaults`).
pub async fn get_settings(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cfg = state.session.effective_config();
    Json(serde_json::json!({
        "settings": {
            "ipv8": {
                "enabled": cfg.ipv8.enabled,
                "address": cfg.ipv8.listen_addr,
                "bootstrap": { "override": cfg.ipv8.bootstrap_peers },
            },
            "tunnel_community": {
                "enabled": cfg.ipv8.enable_anonymity,
                "peer_flags": cfg.ipv8.peer_flags,
                "tribler_community_id": cfg.ipv8.tribler_tunnel_community,
            },
            "libtorrent": {
                "download_defaults": {
                    "saveas": cfg.engine.output_dir.display().to_string(),
                    "anonymity_enabled": false,
                },
                "port": cfg.engine.listen_port,
                "proxy_type": cfg.engine.socks5_proxy.as_ref().map(|_| "socks5"),
                "proxy_server": cfg.engine.socks5_proxy,
                "dht": cfg.engine.enable_dht,
                "utp": true,
            },
            "watch_folder": {
                "enabled": cfg.watch_folder_dir.is_some(),
                "directory": cfg.watch_folder_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            },
            "rss": { "urls": cfg.rss_urls },
            "torrent_checker": { "enabled": cfg.enable_torrent_checker },
            "api": {
                // La cle HTTP est une securite contre les scripts
                // locaux cote Python ; notre API n'accepte que du
                // loopback — pas de cle requise.
                "key": serde_json::Value::Null,
                "http_enabled": true,
            },
            "state_dir": cfg.state_dir.display().to_string(),
        }
    }))
}

/// `POST /api/settings` — mise a jour partielle des reglages
/// applicables a chaud (`update_settings` Python : merge).
///
/// Cles reconnues (meme arbre que `GET`) :
/// - `rss.urls` : liste des flux surveilles
/// - `watch_folder.enabled`/`directory` : repertoire surveille
///
/// Les autres cles sont acceptees mais non appliquees a chaud (un
/// redemarrage est requis — comme plusieurs reglages Python).
pub async fn update_settings(
    State(state): State<AppState>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings = req.get("settings").unwrap_or(&req).clone();
    let mut cfg = state.session.effective_config();

    if let Some(rss) = settings.get("rss") {
        if let Some(urls) = rss.get("urls").and_then(|v| v.as_array()) {
            cfg.rss_urls = urls
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }
    if let Some(wf) = settings.get("watch_folder") {
        let enabled = wf.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let dir = wf
            .get("directory")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from);
        cfg.watch_folder_dir = if enabled { dir } else { None };
    }

    state.session.apply_service_settings(&cfg);
    Ok(Json(serde_json::json!({ "modified": true })))
}
