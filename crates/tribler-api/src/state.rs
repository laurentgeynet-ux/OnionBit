//! Etat partage des handlers (equivalent de ce que les endpoints
//! Python recuperent depuis `request.app`).

use tribler_core::CoreSession;

/// Etat injecte dans tous les handlers.
#[derive(Clone)]
pub struct AppState {
    /// Session coeur (moteur + base + notifier).
    pub session: CoreSession,
}
