// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signal d'arrêt unique du daemon.
//!
//! Trois sources déclenchent la même séquence d'arrêt propre :
//! Ctrl-C (console attachée), l'item « Quitter » du systray et
//! `PUT /api/shutdown`. `Notify` mémorise un permis : un `trigger()`
//! émis avant le premier `wait()` n'est pas perdu.

use std::sync::Arc;

use tokio::sync::{watch, Notify};

/// Source unique de vérité pour « le daemon doit se terminer ».
#[derive(Clone)]
pub struct ShutdownSignal {
    inner: Arc<Notify>,
    /// Drapeau persistant : `Notify` ne reveille qu'un attendant par
    /// permis (`notify_one` du handler `/api/shutdown`) — le premier
    /// attendant touche relaie le drapeau pour tous les autres.
    flag: watch::Sender<bool>,
    /// Sans receveur le canal `watch` est clos et `send` perd la
    /// valeur — en garder un maintient le drapeau vivant.
    _flag_rx: Arc<watch::Receiver<bool>>,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        let (flag, rx) = watch::channel(false);
        Self {
            inner: Arc::new(Notify::new()),
            flag,
            _flag_rx: Arc::new(rx),
        }
    }
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclenche l'arrêt (idempotent).
    #[cfg(any(windows, test))]
    pub fn trigger(&self) {
        let _ = self.flag.send(true);
        self.inner.notify_one();
    }

    /// Attend le déclenchement — resoluble plusieurs fois : apres le
    /// premier declenchement tout appel suivant rend immediatement.
    /// Le premier attendant reveille par `notify_one` propage le
    /// drapeau aux autres (le handler `/api/shutdown` ne touche que
    /// `inner`).
    pub async fn wait(&self) {
        let mut rx = self.flag.subscribe();
        if *rx.borrow_and_update() {
            return;
        }
        tokio::select! {
            _ = rx.changed() => {}
            _ = self.inner.notified() => {
                let _ = self.flag.send(true);
            }
        }
    }

    /// Handle brut pour `AppState` (le handler `/api/shutdown` notifie
    /// à travers lui — `onionbit-api` ne connaît pas ce type).
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

    /// Regression : `notify_one` ne reveille qu'un seul attendant —
    /// le premier touche relaie le drapeau pour tous les autres.
    #[tokio::test]
    async fn wait_reveille_tous_les_attendants_via_notify_brut() {
        let signal = ShutdownSignal::new();
        let s1 = signal.clone();
        let s2 = signal.clone();
        let w1 = tokio::spawn(async move { s1.wait().await });
        let w2 = tokio::spawn(async move { s2.wait().await });
        tokio::task::yield_now().await;
        // Le handler /api/shutdown notifie a travers `notifier()`
        // (un seul permis `notify_one`).
        signal.notifier().notify_one();
        for w in [w1, w2] {
            tokio::time::timeout(std::time::Duration::from_secs(1), w)
                .await
                .expect("chaque attendant doit etre reveille")
                .expect("le spawn ne doit pas paniquer");
        }
    }

    /// `trigger()` positionne le drapeau : tous les attendants et les
    /// appels ulterieurs de `wait()` se resolvent.
    #[tokio::test]
    async fn trigger_reveille_tous_les_attendants_et_reste_latch() {
        let signal = ShutdownSignal::new();
        let s1 = signal.clone();
        let s2 = signal.clone();
        let w1 = tokio::spawn(async move { s1.wait().await });
        let w2 = tokio::spawn(async move { s2.wait().await });
        tokio::task::yield_now().await;
        signal.trigger();
        for w in [w1, w2] {
            tokio::time::timeout(std::time::Duration::from_secs(1), w)
                .await
                .expect("chaque attendant doit etre reveille")
                .expect("le spawn ne doit pas paniquer");
        }
        // Appel tardif : le drapeau latch rend immediatement.
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.wait())
            .await
            .expect("wait() apres trigger doit etre immediat");
    }
}
