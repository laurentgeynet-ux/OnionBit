// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Construction du routeur axum.
//!
//! Les chemins suivent `tribler.core.restapi` (`/api/...`). Le serveur
//! ne doit etre expose que sur `127.0.0.1` (API de controle locale) —
//! voir `onionbit-network-policy`.
//!
//! Quand `AppState::web_ui_dir` est renseigne, les statiques de
//! l'interface web Flutter sont servies en fallback hors `/api` —
//! exemptees d'authentification comme les chemins `/ui`/`/static` de
//! l'`ApiKeyMiddleware` Python (`webui.rs`).

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post, put};
use axum::Router;

use crate::error::ApiError;
use crate::handlers::*;
use crate::state::AppState;

/// Limite de taille du corps HTTP : `MAX_REQUEST_SIZE` Python =
/// `16 * 1024**2` (`rest_endpoint.py`).
const MAX_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Construit le routeur complet (parite `tribler.core.restapi` autant
/// que le backend le permet) : `/api/*` derriere `api_key_auth` +
/// statiques de l'UI web en fallback quand configurees.
pub fn build(state: AppState) -> Router {
    // `/api` nu n'est pas couvert par `nest("/api", …)` : route
    // explicite pour la parite 401/404 (sinon le fallback UI web
    // repondrait index.html).
    let mut app = Router::new()
        .route("/api", any(api_not_found))
        .nest("/api", api_router(state.clone()));
    if let Some(dir) = &state.web_ui_dir {
        app = app.merge(crate::webui::router(
            dir,
            state.api_key.as_deref(),
            state.web_ui_inject_key,
        ));
    }
    app.with_state(state)
}

/// Chemin inconnu sous `/api` : parite `ApiKeyMiddleware` — 401 sans
/// cle valide (meme sur les 404 en Python), 404 sinon. Le fallback du
/// routeur neste n'est pas couvert par la couche `api_key_auth`,
/// d'ou cette route fourre-tout.
async fn api_not_found(State(state): State<AppState>, req: Request<Body>) -> Response {
    if let Some(expected) = state.api_key.as_deref().filter(|k| !k.is_empty()) {
        if !crate::auth::key_matches(&req, expected) {
            return ApiError::unauthorized().into_response();
        }
    }
    ApiError::not_found("Not found").into_response()
}

/// Gate identitaire (ADR-0016, etape 48d) : tant que la session est
/// `identity_pending` (premier boot sous gate) ou `locked` (graine
/// `OBSK`), seules les routes du cycle de vie repondent — toute
/// route identitaire recoit un `409` uniforme. Ainsi aucun acces
/// session ne precede l'existence d'une identite, et l'interface
/// peut conduire l'onboarding sur une API vivante.
async fn identity_gate(
    State(state): State<AppState>,
    req: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    use onionbit_core::session::IdentityPhase;
    let phase = state.session.identity_phase();
    if phase == IdentityPhase::Ready {
        return next.run(req).await;
    }
    // Routes utilisables avant resolution : celles qui servent a
    // resoudre (identite), regler (settings), observer l'etat
    // (events, statistiques tribler, logs) ou arreter (shutdown).
    const ALLOWED: &[&str] = &[
        "/api/identity",
        "/api/settings",
        "/api/shutdown",
        "/api/events",
        "/api/statistics/tribler",
        "/api/logging",
    ];
    // `nest("/api", …)` retire le prefixe de `req.uri()` dans le
    // routeur neste — `OriginalUri` conserve le chemin complet.
    let path = req
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map_or_else(|| req.uri().path().to_string(), |u| u.path().to_string());
    if ALLOWED
        .iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
    {
        return next.run(req).await;
    }
    let message = match phase {
        IdentityPhase::Pending => "identity_pending",
        IdentityPhase::Locked => "identity_locked",
        IdentityPhase::Ready => unreachable!(),
    };
    ApiError::conflict(message).into_response()
}

/// Routeur des endpoints REST/SSE — la couche `api_key_auth` s'applique
/// a toutes les requetes sous `/api`, y compris les chemins inconnus
/// (`ApiKeyMiddleware` Python : une requete non authentifiee vers un
/// chemin inconnu recoit 401, pas 404).
fn api_router(state: AppState) -> Router<AppState> {
    Router::new()
        .layer(DefaultBodyLimit::max(MAX_BODY_LIMIT_BYTES))
        // -- Downloads (downloads_endpoint.py) -------------------------
        .route("/downloads", get(downloads::get_downloads))
        .route("/downloads", put(downloads::add_download))
        .route("/downloads/{infohash}", delete(downloads::delete_download))
        .route("/downloads/{infohash}", patch(downloads::update_download))
        .route(
            "/downloads/{infohash}/torrent",
            get(downloads_extra::get_download_torrent),
        )
        .route(
            "/downloads/{infohash}/clone_public",
            post(downloads_extra::clone_public),
        )
        .route(
            "/downloads/{infohash}/trackers",
            get(downloads_extra::get_trackers),
        )
        .route(
            "/downloads/{infohash}/trackers",
            put(downloads_extra::add_tracker).delete(downloads_extra::remove_tracker),
        )
        .route(
            "/downloads/{infohash}/default_trackers",
            put(downloads_extra::add_default_trackers),
        )
        .route(
            "/downloads/{infohash}/tracker_force_announce",
            put(downloads_extra::tracker_force_announce),
        )
        .route(
            "/downloads/{infohash}/files",
            get(downloads_extra::get_download_files),
        )
        .route(
            "/downloads/{infohash}/stream/{fileindex}",
            get(downloads_extra::stream_file),
        )
        .route("/downloads/clierrors", get(downloads::get_cli_errors))
        // -- Zone privee ADR-0018 (etape 62) : etat `locked|mounted|
        // guest` + catalogue `manifest.obm` dechiffre (reserve au
        // titulaire — la route est derriere `api_key_auth`).
        .route("/private", get(downloads::get_private))
        .route(
            "/private/orphans",
            axum::routing::delete(downloads::purge_private_orphans),
        )
        // Explorateur prive + export en clair (ADR-0027, etape
        // 109) : `key` = row_key opaque ou infohash reel.
        .route("/private/{key}/files", get(downloads::get_private_files))
        .route(
            "/private/{key}/export",
            post(downloads::post_private_export),
        )
        // -- Evenements SSE (events_endpoint.py) ------------------------
        .route("/events", get(events::get_events))
        .route("/events/info", get(events::get_events_info))
        // -- Settings / shutdown / statistiques -------------------------
        .route("/settings", get(settings::get_settings))
        .route("/settings", post(settings::update_settings))
        .route("/shutdown", put(shutdown::shutdown))
        .route("/statistics/tribler", get(statistics::get_onionbit_stats))
        .route("/statistics/ipv8", get(statistics::get_ipv8_stats))
        // Agregat endpoints distants — extension Rust de diagnostic
        // (sans equivalent `tribler.core.restapi`).
        .route("/connections", get(connections::get_connections))
        .route(
            "/statistics/dirspace",
            get(statistics::get_dirspace_stats).put(statistics::put_dirspace_stats),
        )
        // Profils d'anonymat predefinis (ADR-0022 — extension Rust ;
        // `PUT` refuse en session invitee, `GET` libre ; le gate
        // identitaire couvre la route hors whitelist).
        .route(
            "/privacy/profile",
            get(privacy::get_profile).put(privacy::put_profile),
        )
        // -- Metadata / recherche (database_endpoint.py) ----------------
        .route(
            "/metadata/torrents/popular",
            get(metadata::get_popular_torrents),
        )
        .route(
            "/metadata/torrents/health",
            get(metadata::get_torrent_health_history),
        )
        .route(
            "/metadata/torrents/{infohash}/health",
            get(metadata::get_torrent_health),
        )
        .route("/metadata/torrents/{infohash}/tags", put(metadata::add_tag))
        .route(
            "/metadata/torrents/{infohash}/tags",
            delete(metadata::remove_tag),
        )
        .route(
            "/metadata/torrents/{infohash}/tags",
            patch(metadata::update_tags),
        )
        .route("/metadata/search/local", get(metadata::local_search))
        .route("/metadata/search/completions", get(metadata::completions))
        .route("/metadata/search/vocabulary", get(metadata::vocabulary))
        // -- Recherche distante (search_endpoint.py) --------------------
        .route("/search/remote", put(search::remote_search))
        // -- Canaux suivis (channels_endpoint.py — ADR-0025 etape 97) ---
        .route("/channels", get(channels::list_channels))
        .route("/channels/personal", put(channels::personal_set_title))
        .route(
            "/channels/personal/{infohash}",
            delete(channels::personal_remove),
        )
        .route(
            "/channels/personal/{infohash}/commit",
            put(channels::personal_commit),
        )
        .route("/channels/{pk}/{id}", get(channels::channel_contents))
        .route(
            "/channels/{pk}/{id}/subscribe",
            put(channels::subscribe).delete(channels::unsubscribe),
        )
        // -- Info torrent / creation ------------------------------------
        .route("/torrentinfo/uri", post(torrentinfo::get_torrent_info))
        .route(
            "/torrentinfo/file",
            put(torrentinfo::get_torrent_info_from_file),
        )
        .route("/createtorrent", post(createtorrent::create_torrent))
        .route("/createtorrent/dryrun", post(createtorrent::dry_run))
        // -- Session moteur (libtorrent_endpoint.py) --------------------
        .route(
            "/libtorrent/settings",
            get(libtorrent::get_libtorrent_settings),
        )
        .route(
            "/libtorrent/session",
            get(libtorrent::get_libtorrent_session_info),
        )
        // -- IPv8 / tunnel (ipv8_endpoint.py) ---------------------------
        .route("/ipv8/overlays", get(ipv8::get_overlays))
        .route(
            "/ipv8/overlays/statistics",
            get(ipv8::get_overlay_statistics).post(ipv8::post_overlay_statistics),
        )
        .route("/ipv8/network", get(ipv8::get_network))
        .route("/ipv8/isolation", post(ipv8::post_isolation))
        .route("/ipv8/noblockdht/{mid}", get(ipv8::get_noblock_dht))
        .route("/ipv8/tunnel/settings", get(ipv8::get_tunnel_settings))
        .route("/ipv8/tunnel/circuits", get(ipv8::get_tunnel_circuits))
        .route("/ipv8/tunnel/relays", get(ipv8::get_tunnel_relays))
        .route("/ipv8/tunnel/exits", get(ipv8::get_tunnel_exits))
        .route("/ipv8/tunnel/swarms", get(ipv8::get_tunnel_swarms))
        .route(
            "/ipv8/tunnel/swarms/{infohash}/size",
            get(ipv8::get_swarm_size),
        )
        .route("/ipv8/tunnel/peers", get(ipv8::get_tunnel_peers))
        .route("/ipv8/tunnel/ledger", get(ipv8::get_tunnel_ledger))
        // Communaute d'extension OnionBit-only (ADR-0015 — extension
        // Rust ; `enabled:false` tant que `ext/enabled` est off, les
        // sous-routes operatoires repondent 404).
        .route("/ipv8/ext", get(ipv8::get_ext))
        // Transport furtif (ADR-0017 — extension Rust ; compteurs et
        // etat seulement, jamais de cle ni d'adresse de pont).
        .route("/stealth", get(stealth::get_stealth))
        .route("/stealth/bridges", post(stealth::add_bridge))
        // Identite portable — export/import de la cle secrete
        // (blob OBID argon2id+AEAD si mot de passe ; restart requis).
        .route("/identity", get(identity::get_identity))
        .route("/identity/recovery_phrase", get(identity::recovery_phrase))
        .route("/identity/export", post(identity::export_identity))
        .route("/identity/restore", post(identity::restore_identity))
        // Cycle de vie ADR-0016 (etape 48d) : resolution du gate de
        // premier boot, deverrouillage at-rest, session invitee.
        .route("/identity/unlock", post(identity::unlock_identity))
        .route("/identity/create", post(identity::create_identity))
        .route("/identity/guest", post(identity::guest_identity))
        .route("/identity/at_rest", post(identity::set_at_rest))
        // Appairage mobile par QR (ADR-0021 §8) : `token` derriere la
        // cle (le desktop est authentifie) ; `redeem` est exemptee
        // dans `auth.rs` — le mobile vient chercher la cle.
        .route("/pairing/token", post(pairing::post_token))
        .route("/pairing/redeem", post(pairing::post_redeem))
        .route("/ipv8/ext/attest", post(ipv8::post_ext_attest))
        .route("/ipv8/ext/attestations", get(ipv8::get_ext_attestations))
        .route("/ipv8/ext/ledger", get(ipv8::get_ext_ledger))
        .route("/ipv8/ext/trust/{kind}/{subject}", get(ipv8::get_ext_trust))
        // Messagerie e2e (ADR-0011 — extension Rust, pas de parite
        // Python ; tout repond 404 quand `enable_messaging` est off).
        .route("/messaging/stats", get(messaging::get_stats))
        .route("/messaging/contacts", get(messaging::get_contacts))
        .route(
            "/messaging/contacts/{pk}",
            delete(messaging::delete_contact),
        )
        .route("/messaging/contacts/pending", get(messaging::get_pending))
        .route("/messaging/contacts/connect", post(messaging::post_connect))
        .route(
            "/messaging/contacts/{pk}/accept",
            post(messaging::post_accept),
        )
        .route(
            "/messaging/contacts/{pk}/refuse",
            post(messaging::post_refuse),
        )
        .route(
            "/messaging/contacts/{pk}/block",
            post(messaging::post_block).delete(messaging::delete_block),
        )
        .route(
            "/messaging/contacts/{pk}/messages",
            get(messaging::get_messages).post(messaging::post_message),
        )
        .route(
            "/messaging/contacts/{pk}/alias",
            post(messaging::post_alias),
        )
        .route(
            "/messaging/contacts/{pk}/retention",
            post(messaging::post_retention),
        )
        .route(
            "/messaging/messages/{id}",
            delete(messaging::delete_message),
        )
        .route("/messaging/events", get(messaging::get_events))
        // ADR-0019 : conversations / groupes / pieces jointes
        // (aliases v1 conserves : `contacts/{pk}/messages` = conv
        // directe).
        .route(
            "/messaging/conversations",
            get(messaging::get_conversations),
        )
        .route(
            "/messaging/conversations/direct/{pk}",
            get(messaging::get_direct_conversation),
        )
        .route(
            "/messaging/conversations/{conv}/messages",
            get(messaging::get_conv_messages).post(messaging::post_conv_message),
        )
        .route(
            "/messaging/conversations/{conv}/read",
            post(messaging::post_conv_read),
        )
        .route(
            "/messaging/conversations/{conv}",
            delete(messaging::delete_conversation),
        )
        .route(
            "/messaging/conversations/{conv}/attachments",
            get(messaging::get_conv_attachments).post(messaging::post_conv_attach),
        )
        .route("/messaging/groups", post(messaging::post_group))
        .route(
            "/messaging/groups/{conv}/members",
            get(messaging::get_group_members),
        )
        .route(
            "/messaging/groups/{conv}/invite",
            post(messaging::post_group_invite),
        )
        .route(
            "/messaging/groups/{conv}/accept",
            post(messaging::post_group_accept),
        )
        .route(
            "/messaging/groups/{conv}/decline",
            post(messaging::post_group_decline),
        )
        .route(
            "/messaging/groups/{conv}/leave",
            post(messaging::post_group_leave),
        )
        // Uploads : `DefaultBodyLimit` global desactive — la borne
        // effective est `attach_max_bytes` relue a l'execution dans
        // le handler (lecture `to_bytes` bornee).
        .route(
            "/messaging/uploads",
            post(messaging::post_upload).route_layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/messaging/attachments/{id}/accept",
            post(messaging::post_attach_accept),
        )
        .route(
            "/messaging/attachments/{id}/decline",
            post(messaging::post_attach_decline),
        )
        // Coffre de contacts chiffre pour soi (ADR-0015/ADR-0016) —
        // export/import portable entre devices de meme identite.
        .route("/messaging/vault/export", get(messaging::get_vault_export))
        .route(
            "/messaging/vault/import",
            post(messaging::post_vault_import),
        )
        // Coffre replique sur les ponts `CAP_PULL_STORE`
        // (ADR-0026 : VAULT_PUT multi-ponts, VAULT_GET + import).
        .route(
            "/messaging/vault/replicate",
            post(messaging::post_vault_replicate),
        )
        .route(
            "/messaging/vault/restore",
            post(messaging::post_vault_restore),
        )
        .route("/ipv8/tunnel/guards", get(ipv8::get_tunnel_guards))
        .route("/ipv8/tunnel/peers/dht", get(ipv8::get_dht_peers))
        .route("/ipv8/tunnel/peers/pex", get(ipv8::get_pex_peers))
        .route(
            "/ipv8/tunnel/debug/circuit-downloads",
            get(ipv8::get_tunnel_circuit_downloads),
        )
        .route(
            "/ipv8/tunnel/anon_lanes/{hops}",
            delete(ipv8::delete_anon_lane),
        )
        .route(
            "/ipv8/tunnel/circuits/test",
            get(ipv8::speed_test_new_circuit),
        )
        .route(
            "/ipv8/tunnel/circuits/{circuit_id}/test",
            get(ipv8::speed_test_existing_circuit),
        )
        // -- DHT IPv8 (dht_endpoint.py, pyipv8) -------------------------
        .route("/ipv8/dht/statistics", get(dht::get_statistics))
        .route("/ipv8/dht/values", get(dht::get_stored_values))
        .route("/ipv8/dht/values/{key}", get(dht::get_values))
        .route("/ipv8/dht/values/{key}", put(dht::put_value))
        .route("/ipv8/dht/peers/{mid}", get(dht::get_peer))
        .route("/ipv8/dht/buckets", get(dht::get_buckets))
        .route(
            "/ipv8/dht/buckets/{prefix}/refresh",
            get(dht::refresh_bucket),
        )
        // -- Navigateur de fichiers (file_endpoint.py) ------------------
        .route("/files/browse", get(files::browse))
        .route("/files/list", get(files::list))
        .route("/files/create", get(files::create))
        // -- Asyncio (asyncio_endpoint.py, pyipv8) ----------------------
        .route(
            "/ipv8/asyncio/drift",
            get(asyncio::get_drift).put(asyncio::set_drift),
        )
        .route("/ipv8/asyncio/tasks", get(asyncio::get_tasks))
        .route(
            "/ipv8/asyncio/debug",
            get(asyncio::get_debug).put(asyncio::set_debug),
        )
        // -- RSS --------------------------------------------------------
        // `GET` = extension Rust (listing `rss_items`) — Python n'a
        // que `PUT` (cf. ADR-0006).
        .route("/rss", put(rss::update_feeds).get(rss::list_items))
        // -- Versioning / logging ---------------------------------------
        .route("/versioning/versions", get(versioning::get_versions))
        .route(
            "/versioning/versions/current",
            get(versioning::get_current_version),
        )
        .route("/versioning/versions/check", get(versioning::check_version))
        .route(
            "/versioning/versions/{version}",
            delete(versioning::remove_version),
        )
        .route("/versioning/upgrade", post(versioning::perform_upgrade))
        .route(
            "/versioning/upgrade/available",
            get(versioning::can_upgrade),
        )
        .route("/versioning/upgrade/working", get(versioning::is_upgrading))
        .route("/logging", get(logging::get_logs))
        // Route fourre-tout : `/{*rest}` couvre `/api/` et tout chemin
        // inconnu — le fallback interne echapperait a la couche
        // d'auth (parite `ApiKeyMiddleware` : 401 avant routage).
        .route("/{*rest}", any(api_not_found))
        // Gate identitaire ADR-0016 : applique APRES l'auth (la
        // couche ci-dessous est la plus externe) — une sonde non
        // authentifiee recoit 401 et n'apprend meme pas l'etat
        // `locked`/`pending`.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            identity_gate,
        ))
        // `ApiKeyMiddleware` Python : s'applique a toutes les routes
        // (y compris les 404 — une requete non authentifiee vers un
        // chemin inconnu recoit 401, pas 404).
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::api_key_auth,
        ))
}
