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

use tribler_core::Notification;

use crate::state::AppState;

/// Version emise dans le message `events_start`.
const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `DownloadState` → nom `DownloadStatus` Python (enum
/// `download_state.py`, suffixe `.name` emis dans `status`).
fn py_status(state: &tribler_bittorrent::DownloadState) -> &'static str {
    use tribler_bittorrent::DownloadState as S;
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
    let public_key = state.session.public_key_hex();

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

    let events = BroadcastStream::new(rx).filter_map(move |msg| match msg {
        Ok(n) => notification_to_event(&n, &public_key)
            .map(|(topic, kwargs)| Ok(Event::default().event(topic).data(kwargs.to_string()))),
        // Abonne lent : on saute les evenements perdus (lag).
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });

    // Le garde est capture dans la closure : il vit tant que le flux
    // vit, et est droppe quand le client se deconnecte.
    let stream = initial.chain(events).map(move |ev| {
        let _ = &guard;
        ev
    });
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
}
