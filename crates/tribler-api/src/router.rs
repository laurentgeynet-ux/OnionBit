//! Construction du routeur axum.
//!
//! Les chemins suivent `tribler.core.restapi` (`/api/...`). Le serveur
//! ne doit etre expose que sur `127.0.0.1` (API de controle locale) —
//! voir `tribler-network-policy`.

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;

use crate::handlers::*;
use crate::state::AppState;

/// Limite de taille du corps HTTP : `MAX_REQUEST_SIZE` Python =
/// `16 * 1024**2` (`rest_endpoint.py`).
const MAX_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Construit le routeur complet de l'API (parite
/// `tribler.core.restapi` autant que le backend le permet).
pub fn build(state: AppState) -> Router {
    Router::new()
        .layer(DefaultBodyLimit::max(MAX_BODY_LIMIT_BYTES))
        // -- Downloads (downloads_endpoint.py) -------------------------
        .route("/api/downloads", get(downloads::get_downloads))
        .route("/api/downloads", put(downloads::add_download))
        .route(
            "/api/downloads/{infohash}",
            delete(downloads::delete_download),
        )
        .route(
            "/api/downloads/{infohash}",
            patch(downloads::update_download),
        )
        .route(
            "/api/downloads/{infohash}/torrent",
            get(downloads_extra::get_download_torrent),
        )
        .route(
            "/api/downloads/{infohash}/trackers",
            get(downloads_extra::get_trackers),
        )
        .route(
            "/api/downloads/{infohash}/trackers",
            put(downloads_extra::add_tracker).delete(downloads_extra::remove_tracker),
        )
        .route(
            "/api/downloads/{infohash}/default_trackers",
            put(downloads_extra::add_default_trackers),
        )
        .route(
            "/api/downloads/{infohash}/tracker_force_announce",
            put(downloads_extra::tracker_force_announce),
        )
        .route(
            "/api/downloads/{infohash}/files",
            get(downloads_extra::get_download_files),
        )
        .route(
            "/api/downloads/{infohash}/stream/{fileindex}",
            get(downloads_extra::stream_file),
        )
        .route("/api/downloads/clierrors", get(downloads::get_cli_errors))
        // -- Evenements SSE (events_endpoint.py) ------------------------
        .route("/api/events", get(events::get_events))
        .route("/api/events/info", get(events::get_events_info))
        // -- Settings / shutdown / statistiques -------------------------
        .route("/api/settings", get(settings::get_settings))
        .route("/api/settings", post(settings::update_settings))
        .route("/api/shutdown", put(shutdown::shutdown))
        .route(
            "/api/statistics/tribler",
            get(statistics::get_tribler_stats),
        )
        .route("/api/statistics/ipv8", get(statistics::get_ipv8_stats))
        .route(
            "/api/statistics/dirspace",
            get(statistics::get_dirspace_stats).put(statistics::put_dirspace_stats),
        )
        // -- Metadata / recherche (database_endpoint.py) ----------------
        .route(
            "/api/metadata/torrents/popular",
            get(metadata::get_popular_torrents),
        )
        .route(
            "/api/metadata/torrents/health",
            get(metadata::get_torrent_health_history),
        )
        .route(
            "/api/metadata/torrents/{infohash}/health",
            get(metadata::get_torrent_health),
        )
        .route(
            "/api/metadata/torrents/{infohash}/tags",
            put(metadata::add_tag),
        )
        .route(
            "/api/metadata/torrents/{infohash}/tags",
            delete(metadata::remove_tag),
        )
        .route(
            "/api/metadata/torrents/{infohash}/tags",
            patch(metadata::update_tags),
        )
        .route("/api/metadata/search/local", get(metadata::local_search))
        .route(
            "/api/metadata/search/completions",
            get(metadata::completions),
        )
        .route("/api/metadata/search/vocabulary", get(metadata::vocabulary))
        // -- Recherche distante (search_endpoint.py) --------------------
        .route("/api/search/remote", put(search::remote_search))
        // -- Info torrent / creation ------------------------------------
        .route("/api/torrentinfo/uri", post(torrentinfo::get_torrent_info))
        .route(
            "/api/torrentinfo/file",
            put(torrentinfo::get_torrent_info_from_file),
        )
        .route("/api/createtorrent", post(createtorrent::create_torrent))
        .route("/api/createtorrent/dryrun", post(createtorrent::dry_run))
        // -- Session moteur (libtorrent_endpoint.py) --------------------
        .route(
            "/api/libtorrent/settings",
            get(libtorrent::get_libtorrent_settings),
        )
        .route(
            "/api/libtorrent/session",
            get(libtorrent::get_libtorrent_session_info),
        )
        // -- IPv8 / tunnel (ipv8_endpoint.py) ---------------------------
        .route("/api/ipv8/overlays", get(ipv8::get_overlays))
        .route(
            "/api/ipv8/overlays/statistics",
            get(ipv8::get_overlay_statistics).post(ipv8::post_overlay_statistics),
        )
        .route("/api/ipv8/network", get(ipv8::get_network))
        .route("/api/ipv8/isolation", post(ipv8::post_isolation))
        .route("/api/ipv8/noblockdht/{mid}", get(ipv8::get_noblock_dht))
        .route("/api/ipv8/tunnel/settings", get(ipv8::get_tunnel_settings))
        .route("/api/ipv8/tunnel/circuits", get(ipv8::get_tunnel_circuits))
        .route("/api/ipv8/tunnel/relays", get(ipv8::get_tunnel_relays))
        .route("/api/ipv8/tunnel/exits", get(ipv8::get_tunnel_exits))
        .route("/api/ipv8/tunnel/swarms", get(ipv8::get_tunnel_swarms))
        .route(
            "/api/ipv8/tunnel/swarms/{infohash}/size",
            get(ipv8::get_swarm_size),
        )
        .route("/api/ipv8/tunnel/peers", get(ipv8::get_tunnel_peers))
        .route("/api/ipv8/tunnel/peers/dht", get(ipv8::get_dht_peers))
        .route("/api/ipv8/tunnel/peers/pex", get(ipv8::get_pex_peers))
        .route(
            "/api/ipv8/tunnel/circuits/test",
            get(ipv8::speed_test_new_circuit),
        )
        .route(
            "/api/ipv8/tunnel/circuits/{circuit_id}/test",
            get(ipv8::speed_test_existing_circuit),
        )
        // -- DHT IPv8 (dht_endpoint.py, pyipv8) -------------------------
        .route("/api/ipv8/dht/statistics", get(dht::get_statistics))
        .route("/api/ipv8/dht/values", get(dht::get_stored_values))
        .route("/api/ipv8/dht/values/{key}", get(dht::get_values))
        .route("/api/ipv8/dht/values/{key}", put(dht::put_value))
        .route("/api/ipv8/dht/peers/{mid}", get(dht::get_peer))
        .route("/api/ipv8/dht/buckets", get(dht::get_buckets))
        .route(
            "/api/ipv8/dht/buckets/{prefix}/refresh",
            get(dht::refresh_bucket),
        )
        // -- Navigateur de fichiers (file_endpoint.py) ------------------
        .route("/api/files/browse", get(files::browse))
        .route("/api/files/list", get(files::list))
        .route("/api/files/create", get(files::create))
        // -- Asyncio (asyncio_endpoint.py, pyipv8) ----------------------
        .route(
            "/api/ipv8/asyncio/drift",
            get(asyncio::get_drift).put(asyncio::set_drift),
        )
        .route("/api/ipv8/asyncio/tasks", get(asyncio::get_tasks))
        .route(
            "/api/ipv8/asyncio/debug",
            get(asyncio::get_debug).put(asyncio::set_debug),
        )
        // -- RSS --------------------------------------------------------
        // `GET` = extension Rust (listing `rss_items`) — Python n'a
        // que `PUT` (cf. ADR-0006).
        .route("/api/rss", put(rss::update_feeds).get(rss::list_items))
        // -- Versioning / logging ---------------------------------------
        .route("/api/versioning/versions", get(versioning::get_versions))
        .route(
            "/api/versioning/versions/current",
            get(versioning::get_current_version),
        )
        .route(
            "/api/versioning/versions/check",
            get(versioning::check_version),
        )
        .route(
            "/api/versioning/versions/{version}",
            delete(versioning::remove_version),
        )
        .route("/api/versioning/upgrade", post(versioning::perform_upgrade))
        .route(
            "/api/versioning/upgrade/available",
            get(versioning::can_upgrade),
        )
        .route(
            "/api/versioning/upgrade/working",
            get(versioning::is_upgrading),
        )
        .route("/api/logging", get(logging::get_logs))
        // `ApiKeyMiddleware` Python : s'applique a toutes les routes
        // (y compris les 404 — une requete non authentifiee vers un
        // chemin inconnu recoit 401, pas 404).
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::api_key_auth,
        ))
        .with_state(state)
}
