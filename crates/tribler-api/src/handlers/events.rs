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
use futures_util::stream::{self, Stream};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use tribler_core::Notification;

use crate::state::AppState;

/// Version emise dans le message `events_start`.
const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Convertit une notification interne en `(topic, kwargs json)` au
/// format Python (`Notification.<name>`).
///
/// Les noms de topics suivent `tribler.core.notifier.Notification`.
fn notification_to_event(n: &Notification) -> Option<(String, serde_json::Value)> {
    let (topic, kwargs) = match n {
        Notification::SessionStarted => (
            "events_start".to_string(),
            serde_json::json!({"public_key": "", "version": API_VERSION, "sessions": "1"}),
        ),
        Notification::SessionStopping => ("tribler_shutdown_started".into(), serde_json::json!({})),
        Notification::DownloadProgress(s) => (
            "download_state_changed".into(),
            serde_json::to_value(crate::dto::DownloadInfo::from_stats(s)).ok()?,
        ),
        Notification::DownloadFinished { infohash, name } => (
            "torrent_finished".into(),
            serde_json::json!({"infohash": infohash, "name": name, "hidden": false}),
        ),
        Notification::DownloadStateChanged { infohash, state } => (
            "download_state_changed".into(),
            serde_json::json!({"infohash": infohash, "status": format!("{state:?}")}),
        ),
        Notification::TorrentMetadataCreated { infohash, title } => (
            "new_torrent_metadata_created".into(),
            serde_json::json!({"infohash": infohash, "title": title}),
        ),
    };
    Some((topic, kwargs))
}

/// `GET /api/events` — ouvre le flux d'evenements SSE.
pub async fn get_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.session.notifier().subscribe();

    // Message initial (equivalent de `initial_message()` Python).
    let initial = stream::once(async move {
        Ok::<_, Infallible>(
            Event::default().event("events_start").data(
                serde_json::json!({
                    "public_key": "",
                    "version": API_VERSION,
                    "sessions": "1",
                })
                .to_string(),
            ),
        )
    });

    let events = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(n) => notification_to_event(&n)
            .map(|(topic, kwargs)| Ok(Event::default().event(topic).data(kwargs.to_string()))),
        // Abonne lent : on saute les evenements perdus (lag).
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });

    Sse::new(initial.chain(events)).keep_alive(KeepAlive::default())
}
