//! Signal d'arrêt unique du daemon.
//!
//! Trois sources déclenchent la même séquence d'arrêt propre :
//! Ctrl-C (console attachée), l'item « Quitter » du systray et
//! `PUT /api/shutdown`. `Notify` mémorise un permis : un `trigger()`
//! émis avant le premier `wait()` n'est pas perdu.

use std::sync::Arc;

use tokio::sync::Notify;

/// Source unique de vérité pour « le daemon doit se terminer ».
#[derive(Clone, Default)]
pub struct ShutdownSignal {
    inner: Arc<Notify>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclenche l'arrêt (idempotent : le surplus de `notify_one` est
    /// sans effet une fois le permis consommé).
    pub fn trigger(&self) {
        self.inner.notify_one();
    }

    /// Attend le déclenchement (consommé par le graceful shutdown axum).
    pub async fn wait(&self) {
        self.inner.notified().await;
    }

    /// Handle brut pour `AppState` (le handler `/api/shutdown` notifie
    /// à travers lui — `tribler-api` ne connaît pas ce type).
    pub fn notifier(&self) -> Arc<Notify> {
        self.inner.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn le_signal_se_propage_et_survit_au_declenchement_anticipe() {
        let signal = ShutdownSignal::new();
        // Trigger avant toute attente : le permis Notify est mémorisé.
        signal.trigger();
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.wait())
            .await
            .expect("wait() doit se résoudre après trigger()");
    }

    #[tokio::test]
    async fn le_signal_attend_sans_declenchement() {
        let signal = ShutdownSignal::new();
        let elapsed =
            tokio::time::timeout(std::time::Duration::from_millis(50), signal.wait()).await;
        assert!(elapsed.is_err(), "wait() ne doit pas se résoudre seul");
    }
}
