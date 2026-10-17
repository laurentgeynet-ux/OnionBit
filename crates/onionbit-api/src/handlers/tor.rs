// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers Tor (ADR-0028 §5) — **extension Rust sans equivalent
//! Python** : Tribler n'a ni recherche `.onion` ni endpoint d'etat
//! Tor.
//!
//! - `GET /api/tor/status` → `{enabled, mode, state, progress}` —
//!   toujours joignable, y compris bootstrap echoue : prouve que le
//!   runtime ne bloque jamais le daemon.
//! - `GET /api/search/external?source=tpb&q=…` → `{uuid, results}`.
//!   Les resultats sont aussi notifies sur `remote_query_results`
//!   (uuid partage) — l'UI les recoit comme les reponses reseau.
//!
//! Invariants :
//!
//! - **aucun terme de recherche dans les logs** (ni dans `tracing`,
//!   ni dans les messages d'erreur) — la requete n'apparait que
//!   dans la notification SSE, canal API authentifie ;
//! - la recherche declenche le bootstrap Tor **explicitement** mais
//!   n'attend jamais sa fin : `bootstrapping` rend 503 immediate,
//!   l'UI repose la question ;
//! - pas de repli : Tor indisponible = erreur typee, jamais un
//!   fetch clearnet.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use onionbit_tor::{ExternalSearchError, TorError, TorState};

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/tor/status` — etat du runtime, toujours repond (meme
/// desactive ou en echec).
pub async fn tor_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let runtime = state.session.tor();
    let settings = runtime.settings();
    let (state_str, progress, error) = match runtime.state() {
        TorState::Off => ("off", 0, None),
        TorState::Bootstrapping { progress } => ("bootstrapping", progress, None),
        TorState::Ready => ("ready", 100, None),
        TorState::Dormant => ("dormant", 100, None),
        TorState::Error(e) => ("error", 0, Some(e)),
    };
    let (mode, socks5) = match &settings.mode {
        onionbit_tor::TorMode::Embedded => ("embedded", None),
        onionbit_tor::TorMode::External(addr) => ("external", Some(addr.to_string())),
    };
    let svc = runtime.onion_service();
    let vault = runtime.onion_vault_service();
    Json(serde_json::json!({
        "enabled": settings.enabled,
        "mode": mode,
        "external_socks5": socks5,
        "state": state_str,
        "progress": progress,
        "error": error,
        "sources": state.session.search_backend_sources(),
        // ADR-0029 etape 122 : `address` n'est renseignee que si le
        // service est reellement publie — jamais d'adresse fictive.
        "onion_service": {
            "enabled": svc.enabled,
            "state": svc.state,
            "address": svc.address,
        },
        // Etape 127 : service coffre dedie (restricted-discovery) —
        // meme regle : adresse seulement quand publiee.
        "onion_vault": {
            "enabled": vault.enabled,
            "state": vault.state,
            "address": vault.address,
        },
    }))
}

/// Parametres de `GET /api/tor/probe`.
#[derive(Debug, Deserialize)]
pub struct TorProbeQuery {
    /// Identifiant du backend (`tor.search.backends.<source>`).
    pub source: Option<String>,
}

/// `GET /api/tor/probe?source=tpb` — sonde de joignabilite des
/// miroirs `.onion` du backend : un `GET /` borne par miroir, en
/// sequence, sur la lane permanente. Indicateur de sante de la
/// source pour l'UI (un onion deprecie/mort est visible sans avoir
/// a lancer une recherche) — jamais de sondage en tache de fond :
/// la sonde ne part qu'a la demande explicite de l'UI.
///
/// - `400` : `source` absent ou inconnu ;
/// - `503` : `tor.enabled=false`, bootstrap en cours ou echoue
///   (filet d'auto-reparation comme `external_search`) ;
/// - `200` : `{source, reachable, mirrors:[{host, ok, status,
///   latency_ms, error}]}` — `ok` = reponse HTTP recuee (n'importe
///   quel statut : le transport onion fonctionne).
pub async fn tor_probe(
    State(state): State<AppState>,
    Query(params): Query<TorProbeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = params
        .source
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("source parameter missing or empty"))?;
    let backend = state.session.search_backend(&source).ok_or_else(|| {
        ApiError::with_code(
            StatusCode::BAD_REQUEST,
            "unknown_source",
            format!("backend de recherche inconnu : {source}"),
        )
    })?;
    let runtime = state.session.tor();
    if !runtime.settings().enabled {
        return Err(tor_unavailable(
            "tor_disabled",
            "connectivite Tor desactivee (tor.enabled=false)",
        ));
    }
    match runtime.state() {
        TorState::Ready => {}
        TorState::Off | TorState::Error(_) => {
            // Filet d'auto-reparation — meme regle que
            // `external_search` : retente le bootstrap plutot que
            // de figer la lane sur un echec.
            runtime.bootstrap().map_err(|e: TorError| {
                tor_unavailable("bootstrap_failed", format!("bootstrap Tor : {e}"))
            })?;
            return Err(tor_unavailable(
                "tor_bootstrapping",
                "bootstrap Tor en cours".to_string(),
            ));
        }
        TorState::Bootstrapping { progress } => {
            return Err(tor_unavailable(
                "tor_bootstrapping",
                format!("bootstrap Tor en cours ({progress}%)"),
            ));
        }
        TorState::Dormant => {
            return Err(tor_unavailable("tor_not_ready", "client Tor dormant"));
        }
    }

    let mut mirrors = Vec::new();
    let mut reachable = 0usize;
    for host in backend.mirrors() {
        let started = std::time::Instant::now();
        match runtime.fetch_onion(&format!("http://{host}/")).await {
            Ok(resp) => {
                reachable += 1;
                mirrors.push(serde_json::json!({
                    "host": host,
                    "ok": true,
                    "status": resp.status,
                    "latency_ms": started.elapsed().as_millis() as u64,
                }));
            }
            Err(e) => {
                mirrors.push(serde_json::json!({
                    "host": host,
                    "ok": false,
                    "status": serde_json::Value::Null,
                    "latency_ms": started.elapsed().as_millis() as u64,
                    "error": e.to_string(),
                }));
            }
        }
    }
    tracing::info!(
        backend = %source,
        reachable,
        total = mirrors.len(),
        "tor : sonde de joignabilite des miroirs"
    );
    Ok(Json(serde_json::json!({
        "source": source,
        "reachable": reachable,
        "mirrors": mirrors,
    })))
}

/// Parametres de `GET /api/search/external`.
#[derive(Debug, Deserialize)]
pub struct ExternalSearchQuery {
    /// Identifiant du backend (`tor.search.backends.<source>`).
    pub source: Option<String>,
    /// Terme de recherche — jamais journalise.
    pub q: Option<String>,
}

/// Projection JSON d'un [`onionbit_tor::ExternalResult`] — meme
/// forme que `TorrentMetadata.to_simple_dict()` (`remote_query_
/// results`) pour fusionner sans adaptation dans la table
/// `TorrentResult` de l'UI, plus `source`/`magnet` explicites.
fn result_json(r: &onionbit_tor::ExternalResult) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "category": r.category,
        "infohash": r.infohash,
        "size": r.size_bytes,
        "num_seeders": r.seeds,
        "num_leechers": r.leechers,
        "last_tracker_check": 0,
        "created": 0,
        "updated": 0,
        "type": "torrent",
        "source": r.source,
        "magnet": r.magnet,
        "trackers": [],
        "tracker_info": "",
    })
}

/// Erreur 503 de disponibilite Tor avec code machine (`error.code`
/// — l'UI distingue `tor_bootstrapping` d'un vrai echec).
fn tor_unavailable(code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::with_code(StatusCode::SERVICE_UNAVAILABLE, code, message)
}

/// Traduit une [`ExternalSearchError`] en erreur API : `400` pour
/// les fautes de requete, `503` pour l'indisponibilite Tor, `502`
/// pour les echecs de backend — toujours avec `error.code`.
fn map_search_error(e: ExternalSearchError) -> ApiError {
    let msg = e.to_string();
    match e {
        ExternalSearchError::EmptyQuery => {
            ApiError::with_code(StatusCode::BAD_REQUEST, "empty_query", msg)
        }
        ExternalSearchError::UnknownSource(_) => {
            ApiError::with_code(StatusCode::BAD_REQUEST, "unknown_source", msg)
        }
        ExternalSearchError::TorDisabled => tor_unavailable("tor_disabled", msg),
        ExternalSearchError::TorNotReady => tor_unavailable("tor_not_ready", msg),
        // Echec de transport apres bascule de tous les miroirs.
        ExternalSearchError::MirrorUnreachable { .. } => {
            ApiError::with_code(StatusCode::BAD_GATEWAY, "mirror_unreachable", msg)
        }
        ExternalSearchError::CaptchaDetected => {
            ApiError::with_code(StatusCode::BAD_GATEWAY, "captcha_detected", msg)
        }
        ExternalSearchError::StructureChanged => {
            ApiError::with_code(StatusCode::BAD_GATEWAY, "structure_changed", msg)
        }
        ExternalSearchError::NoResults => {
            ApiError::with_code(StatusCode::BAD_GATEWAY, "no_results", msg)
        }
    }
}

/// `GET /api/search/external?source=tpb&q=…` — recherche sur les
/// services `.onion` configures (ADR-0028 §5).
///
/// - `400` : `q` absent/vide ou `source` inconnue ;
/// - `503` : `tor.enabled=false`, bootstrap en cours ou echoue —
///   **jamais** de repli clearnet ;
/// - `200` : `{uuid, source, results}` + notification SSE
///   `remote_query_results` (uuid partage, `peer = "tor:<source>"`) ;
/// - `502` : echec backend type (`mirror_unreachable`,
///   `captcha_detected`, `structure_changed`, `no_results`).
pub async fn external_search(
    State(state): State<AppState>,
    Query(params): Query<ExternalSearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let query = params
        .q
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("q parameter missing or empty"))?;
    let source = params
        .source
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("source parameter missing or empty"))?;
    let backend = state.session.search_backend(&source).ok_or_else(|| {
        ApiError::with_code(
            StatusCode::BAD_REQUEST,
            "unknown_source",
            format!("backend de recherche inconnu : {source}"),
        )
    })?;

    // Gate Tor : indisponible → 503 explicite, jamais de repli.
    let runtime = state.session.tor();
    if !runtime.settings().enabled {
        return Err(tor_unavailable(
            "tor_disabled",
            "connectivite Tor desactivee (tor.enabled=false)",
        ));
    }
    match runtime.state() {
        TorState::Ready => {}
        // Filet de la lane permanente (ADR-0028) : `tor.enabled`
        // bootstrappe normalement au demarrage ou a l'activation —
        // on retente ici quand la lane est restee `Off` ou qu'un
        // bootstrap anterieur a echoue. `bootstrap` est non
        // bloquant et idempotent pendant une tentative en cours :
        // chaque recherche peut reparer la lane, jamais la re-armer
        // deux fois.
        TorState::Off | TorState::Error(_) => {
            if let TorState::Error(e) = runtime.state() {
                tracing::info!(error = %e, "tor : relance du bootstrap apres echec");
            }
            runtime.bootstrap().map_err(|e: TorError| {
                tor_unavailable("bootstrap_failed", format!("bootstrap Tor : {e}"))
            })?;
            let progress = match runtime.state() {
                TorState::Bootstrapping { progress } => progress,
                // Cas nominal impossible en pratique (bootstrap
                // pose `Bootstrapping` avant de retourner), mais le
                // runtime est un stub en test : etat force `Ready`.
                TorState::Ready => return search_and_reply(state, backend, source, query).await,
                _ => 0,
            };
            return Err(tor_unavailable(
                "tor_bootstrapping",
                format!("bootstrap Tor en cours ({progress}%)"),
            ));
        }
        TorState::Bootstrapping { progress } => {
            return Err(tor_unavailable(
                "tor_bootstrapping",
                format!("bootstrap Tor en cours ({progress}%)"),
            ));
        }
        TorState::Dormant => {
            return Err(tor_unavailable("tor_not_ready", "client Tor dormant"));
        }
    }

    search_and_reply(state, backend, source, query).await
}

/// Execute la recherche puis publie le resultat — sortie commune
/// des chemins `Ready` direct et bootstrap-deja-pret.
async fn search_and_reply(
    state: AppState,
    backend: std::sync::Arc<dyn onionbit_tor::OnionSearchBackend>,
    source: String,
    query: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let results = backend.search(&query).await.map_err(map_search_error)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    let items: Vec<serde_json::Value> = results.iter().map(result_json).collect();
    // Meme notification que la recherche reseau (`remote_query_
    // results`, uuid partage) — `peer = "tor:<source>"` distingue
    // l'origine sans exposer l'hote onion.
    state
        .session
        .notifier()
        .notify(onionbit_core::Notification::RemoteQueryResults {
            query: query.clone(),
            results: items.clone(),
            uuid: uuid.clone(),
            peer: format!("tor:{source}"),
        });
    // Jamais le terme de recherche dans les logs (ADR-0028 §6).
    tracing::info!(
        %uuid,
        %source,
        count = items.len(),
        "recherche onion externe servie"
    );
    Ok(Json(serde_json::json!({
        "uuid": uuid,
        "source": source,
        "results": items,
    })))
}
