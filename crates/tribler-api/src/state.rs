//! Etat partage des handlers (equivalent de ce que les endpoints
//! Python recuperent depuis `request.app`).

use tribler_core::CoreSession;

/// Etat injecte dans tous les handlers.
#[derive(Clone)]
pub struct AppState {
    /// Session coeur (moteur + base + notifier).
    pub session: CoreSession,
    /// File des erreurs d'ajout CLI non vues
    /// (`unhandled_cli_log` Python : insertions en tete, borne 100,
    /// videe par `GET /api/downloads/clierrors`).
    pub unhandled_cli: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    /// Nombre de clients SSE connectes a `/api/events` (le champ
    /// `sessions` du message `events_start` / `/api/events/info`).
    pub sse_sessions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AppState {
    /// Etat par defaut autour d'une session coeur.
    pub fn new(session: CoreSession) -> Self {
        Self {
            session,
            unhandled_cli: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            sse_sessions: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Pousse une erreur d'ajout dans le journal CLI (borne 100,
    /// le plus recent en tete — comme `_add_err` Python).
    pub fn push_cli_error(&self, msg: impl Into<String>) {
        let mut log = self.unhandled_cli.lock().unwrap();
        log.push_front(msg.into());
        log.truncate(100);
    }
}
