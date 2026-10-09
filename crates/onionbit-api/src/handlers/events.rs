// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handler `/api/events` — flux SSE des notifications internes.
//!
//! Equivalent de `tribler.core.restapi.events_endpoint` : le client
//! recoit des trames `event: <topic>\ndata: <json>\n\n`. Le premier
//! message est `events_start` (cle publique + version + nombre de
//! sessions connectees).
//!
//! Contrairement a Python qui maintient une queue d'evenements par
//! connexion, ici chaque client SSE s'abonne directement au
//! `broadcast::Receiver` du `Notifier` (un client lent recoit
//! `Lagged` et perd les evenements intermediaires — acceptable pour
//! un canal de notification).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::stream::{self, Stream};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use onionbit_core::Notification;

use crate::state::AppState;

/// Version emise dans le message `events_start`.
const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `DownloadState` → nom `DownloadStatus` Python (enum
/// `download_state.py`, suffixe `.name` emis dans `status`).
fn py_status(state: &onionbit_bittorrent::DownloadState) -> &'static str {
    use onionbit_bittorrent::DownloadState as S;
    match state {
        S::Initializing => "METADATA",
        S::Checking => "HASHCHECKING",
        S::Downloading => "DOWNLOADING",
        S::Seeding => "SEEDING",
        S::Paused | S::Stopped => "STOPPED",
        S::Error => "STOPPED_ON_ERROR",
    }
}

/// Convertit une notification interne en `(topic, kwargs json)` au
/// format Python (`Notification.<name>`).
///
/// Les noms de topics suivent `tribler.core.notifier.Notification` ;
/// `download_state_changed` est une extension locale (le GUI Tribler
/// consomme `torrent_status_changed`, pas de topic de progression).
fn notification_to_event(
    n: &Notification,
    public_key: &str,
) -> Option<(String, serde_json::Value)> {
    let (topic, kwargs) = match n {
        Notification::SessionStarted => (
            "events_start".to_string(),
            serde_json::json!({"public_key": public_key, "version": API_VERSION, "sessions": "1"}),
        ),
        Notification::SessionStopping | Notification::ShutdownState { .. } => {
            let state = match n {
                Notification::ShutdownState { state } => state.clone(),
                _ => "Shutting down.".to_string(),
            };
            (
                "tribler_shutdown_state".into(),
                serde_json::json!({"state": state}),
            )
        }
        Notification::DownloadProgress(s) => (
            "download_state_changed".into(),
            serde_json::to_value(crate::dto::DownloadInfo::from_stats(s)).ok()?,
        ),
        Notification::DownloadFinished { infohash, name } => (
            "torrent_finished".into(),
            serde_json::json!({"infohash": infohash, "name": name, "hidden": false}),
        ),
        Notification::DownloadStateChanged { infohash, state } => (
            "torrent_status_changed".into(),
            serde_json::json!({"infohash": infohash, "status": py_status(state)}),
        ),
        Notification::TorrentMetadataCreated { infohash, title } => (
            "new_torrent_metadata_created".into(),
            serde_json::json!({"infohash": infohash, "title": title}),
        ),
        Notification::TorrentHealthUpdated {
            infohash,
            seeders,
            leechers,
            ..
        } => (
            "torrent_health_updated".into(),
            serde_json::json!({
                "infohash": infohash,
                "seeders": seeders,
                "leechers": leechers,
            }),
        ),
        Notification::RemoteQueryResults {
            query,
            results,
            uuid,
            peer,
        } => (
            "remote_query_results".into(),
            serde_json::json!({
                "query": query,
                "results": results,
                "uuid": uuid,
                "peer": peer,
            }),
        ),
        Notification::LocalQueryResults { query, results } => (
            "local_query_results".into(),
            serde_json::json!({"query": query, "results": results}),
        ),
        Notification::TunnelRemoved {
            circuit_id,
            circuit_class,
            bytes_up,
            bytes_down,
            uptime_secs,
            additional_info,
        } => (
            "tunnel_removed".into(),
            serde_json::json!({
                "circuit_id": circuit_id,
                "circuit_class": circuit_class,
                "bytes_up": bytes_up,
                "bytes_down": bytes_down,
                "uptime": uptime_secs,
                "additional_info": additional_info,
            }),
        ),
        Notification::LowSpace { disk_usage_data } => (
            "low_space".into(),
            serde_json::json!({"disk_usage_data": disk_usage_data}),
        ),
        Notification::TriblerException { error } => (
            "tribler_exception".into(),
            serde_json::json!({"error": error, "traceback": ""}),
        ),
        Notification::ReportConfigError { error } => (
            "report_config_error".into(),
            serde_json::json!({"error": error}),
        ),
        Notification::AskAddDownload { uri } => {
            ("ask_add_download".into(), serde_json::json!({"uri": uri}))
        }
        Notification::TriblerNewVersion { version } => (
            "tribler_new_version".into(),
            serde_json::json!({"version": version}),
        ),
        Notification::SettingsChanged => ("settings_changed".into(), serde_json::json!({})),
        Notification::PrivateTorrentDetected { infohash, name } => (
            "private_torrent_detected".into(),
            serde_json::json!({"infohash": infohash, "name": name}),
        ),
    };
    Some((topic, kwargs))
}

/// Garde-compteur : +1 a la connexion SSE, -1 quand le flux est
/// droppe (equivalent de la liste `events_responses` Python).
struct SessionGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Flux SSE complet : message initial, notifications mappees, puis
/// l'evenement d'arret terminal. Sorti de `get_events` pour etre
/// testable sans `AppState`.
fn build_stream(
    rx: tokio::sync::broadcast::Receiver<Notification>,
    public_key: String,
    sessions: usize,
    guard: SessionGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    // Message initial (equivalent de `initial_message()` Python).
    let pk = public_key.clone();
    let initial = stream::once(async move {
        Ok::<_, Infallible>(
            Event::default().event("events_start").data(
                serde_json::json!({
                    "public_key": pk,
                    "version": API_VERSION,
                    "sessions": sessions.to_string(),
                })
                .to_string(),
            ),
        )
    });

    // `SessionStopping` termine le flux : les connexions SSE restent
    // sinon *in-flight* a vie — le graceful shutdown d'axum attend le
    // drain des connexions sans borne et le daemon ne pouvait pas
    // s'arreter tant qu'un client (UI, navigateur) etait connecte.
    let events = BroadcastStream::new(rx)
        .take_while(|msg| !matches!(msg, Ok(Notification::SessionStopping)))
        .filter_map(move |msg| match msg {
            Ok(n) => notification_to_event(&n, &public_key)
                .map(|(topic, kwargs)| Ok(Event::default().event(topic).data(kwargs.to_string()))),
            // Abonne lent : on saute les evenements perdus (lag).
            Err(BroadcastStreamRecvError::Lagged(_)) => None,
        });

    // Le client recoit l'evenement d'arret puis la connexion se
    // ferme, ce qui libere le drain du serveur.
    let stop_event = stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("tribler_shutdown_state")
                .data(serde_json::json!({"state": "Shutting down."}).to_string()),
        )
    });

    // Le garde est capture dans la closure : il vit tant que le flux
    // vit, et est droppe quand le client se deconnecte.
    initial.chain(events).chain(stop_event).map(move |ev| {
        let _ = &guard;
        ev
    })
}

/// `GET /api/events` — ouvre le flux d'evenements SSE.
pub async fn get_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.session.notifier().subscribe();
    let sessions = state
        .sse_sessions
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;
    let guard = SessionGuard(state.sse_sessions.clone());
    // `public_key` Python : cle publique du noeud IPv8 de la session.
    let stream = build_stream(rx, state.session.public_key_hex(), sessions, guard);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /api/events/info` — info generale de l'endpoint
/// (`get_info` Python : retourne les `kwargs` du message initial).
pub async fn get_events_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "public_key": state.session.public_key_hex(),
        "version": API_VERSION,
        "sessions": state
            .sse_sessions
            .load(std::sync::atomic::Ordering::Relaxed)
            .to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `settings_changed` doit atteindre les clients SSE — c'est la
    /// boucle de resynchronisation multi-clients de l'UI.
    #[test]
    fn settings_changed_est_mappe_en_topic_sse() {
        let (topic, kwargs) = notification_to_event(&Notification::SettingsChanged, "aa").unwrap();
        assert_eq!(topic, "settings_changed");
        assert_eq!(kwargs, serde_json::json!({}));
    }

    /// `SessionStopping` doit fermer le flux : sans cela la connexion
    /// SSE reste *in-flight* a vie et le graceful shutdown d'axum
    /// attend le drain des connexions indefiniment.
    #[tokio::test]
    async fn le_flux_sse_se_termine_sur_session_stopping() {
        let (tx, rx) = tokio::sync::broadcast::channel::<Notification>(8);
        let compteur = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let flux = build_stream(rx, "aa".to_string(), 1, SessionGuard(compteur));

        tx.send(Notification::SettingsChanged).ok();
        tx.send(Notification::SessionStopping).ok();
        // Apres SessionStopping : ne doit jamais etre livre.
        tx.send(Notification::SettingsChanged).ok();

        let evenements =
            tokio::time::timeout(std::time::Duration::from_secs(5), flux.collect::<Vec<_>>())
                .await
                .expect("le flux doit se terminer apres SessionStopping");
        // events_start + settings_changed + evenement d'arret final.
        assert_eq!(evenements.len(), 3);
    }
}
