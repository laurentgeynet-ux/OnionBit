// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `AsyncioMonitor` — etat mutable de l'endpoint `/api/ipv8/asyncio` :
//! mesure de derive (`DriftMeasurementStrategy` Python) et acces au
//! registre de taches nommees.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::tasks::{now_secs, TaskRegistry};

/// `deque(maxlen=100)` de `DriftMeasurementStrategy.history`.
const DRIFT_HISTORY_CAPACITY: usize = 100;

/// Periode de la sonde de lag de l'executor Tokio.
const LAG_PROBE_PERIOD: Duration = Duration::from_millis(100);
/// Retard au-dela duquel la sonde log un `warn!` — un tick qui arrive
/// en retard de cette ampleur signifie qu'une tache bloque l'executor
/// (typiquement un appel `std::fs` synchrone dans du code async).
const LAG_WARN_THRESHOLD: Duration = Duration::from_millis(500);

/// Mesure de derive en cours (`self.strategy` Python quand enable).
struct DriftMeasurement {
    /// `(timestamp, drift)` — drift = `max(0, reel - attendu)`.
    history: Arc<Mutex<VecDeque<(f64, f64)>>>,
    /// Arret de la tache de mesure.
    abort: tokio::task::AbortHandle,
}

/// Etat de l'endpoint asyncio (drift + registre de taches).
///
/// Toujours present sur la session ; le drift exige une stack IPv8
/// (le check `self.session` Python est fait dans le handler REST).
pub struct AsyncioMonitor {
    /// `strategy` — `None` tant que `PUT /drift {"enable": true}`
    /// n'a pas ete appele (ou apres `disable`).
    drift: Mutex<Option<DriftMeasurement>>,
    /// `all_tasks()` : registre des taches nommees du daemon.
    pub tasks: TaskRegistry,
    /// `self.session.walk_interval` Python (s).
    walker_interval: f64,
    /// Sonde de lag de l'executor (diagnostic perf — voir
    /// [`spawn_lag_probe`]). Abordee au `Drop` du moniteur.
    lag_probe: tokio::task::AbortHandle,
}

impl std::fmt::Debug for AsyncioMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncioMonitor")
            .field("drift_enabled", &self.drift.lock().unwrap().is_some())
            .field("walker_interval", &self.walker_interval)
            .finish_non_exhaustive()
    }
}

impl AsyncioMonitor {
    /// Cree le moniteur (`walker_interval` = tick IPv8, en secondes).
    ///
    /// Doit etre appele dans un contexte Tokio : la sonde de lag de
    /// l'executor demarre immediatement.
    pub fn new(walker_interval: f64) -> Self {
        Self {
            drift: Mutex::new(None),
            tasks: TaskRegistry::default(),
            walker_interval,
            lag_probe: spawn_lag_probe(LAG_PROBE_PERIOD, LAG_WARN_THRESHOLD),
        }
    }

    /// `enable()` : enregistre la strategie de mesure — ici, lance
    /// la tache `interval(walker_interval)`. Idempotent.
    pub fn enable_drift(&self) -> bool {
        let mut drift = self.drift.lock().unwrap();
        if drift.is_some() {
            return true;
        }
        let history = Arc::new(Mutex::new(VecDeque::with_capacity(DRIFT_HISTORY_CAPACITY)));
        let shared = history.clone();
        let expected = Duration::from_secs_f64(self.walker_interval.max(0.0));
        let expected_secs = self.walker_interval.max(0.0);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(expected);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last = now_secs();
            loop {
                tick.tick().await;
                let t = now_secs();
                let mut h = shared.lock().unwrap();
                if h.len() >= DRIFT_HISTORY_CAPACITY {
                    h.pop_front();
                }
                // `max(0.0, this_time - last - core_update_rate)` Python.
                h.push_back((t, (t - last - expected_secs).max(0.0)));
                last = t;
            }
        });
        *drift = Some(DriftMeasurement {
            history,
            abort: task.abort_handle(),
        });
        true
    }

    /// `disable()` : retire la strategie — arret de la tache et perte
    /// de l'historique (`strategy = None` Python → 404 sur GET).
    pub fn disable_drift(&self) -> bool {
        if let Some(m) = self.drift.lock().unwrap().take() {
            m.abort.abort();
        }
        true
    }

    /// `strategy.history` — `None` quand la mesure est desactivee.
    pub fn drift_history(&self) -> Option<Vec<(f64, f64)>> {
        self.drift
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.history.lock().unwrap().iter().copied().collect())
    }
}

impl Drop for AsyncioMonitor {
    fn drop(&mut self) {
        self.lag_probe.abort();
    }
}

/// Sonde de lag de l'executor Tokio : tick toutes les `period` et
/// compare l'instant reel au prevu. Un retard superieur a
/// `warn_threshold` produit un `warn!` avec la derive mesuree — c'est
/// le symptome direct d'une tache qui monopolise le runtime (I/O
/// synchrone, calcul lourd sans yield).
pub fn spawn_lag_probe(period: Duration, warn_threshold: Duration) -> tokio::task::AbortHandle {
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut expected = std::time::Instant::now();
        loop {
            expected += period;
            tick.tick().await;
            let lag = expected.elapsed();
            if lag >= warn_threshold {
                tracing::warn!(
                    lag_ms = lag.as_millis() as u64,
                    "lag executor tokio — tache bloquante suspectee"
                );
            }
            expected = std::time::Instant::now();
        }
    });
    task.abort_handle()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drift_enable_history_disable() {
        // Walker interval minuscule pour un test rapide.
        let mon = AsyncioMonitor::new(0.01);
        assert!(mon.drift_history().is_none());
        assert!(mon.enable_drift());
        // Poll jusqu'a 2 s : un sleep fixe de 50 ms est flaky quand la
        // machine est chargee (la tache spawnée n'est pas ordonnan-cée
        // assez vite pour produire un tick de 10 ms).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let history = loop {
            let h = mon.drift_history().unwrap();
            if !h.is_empty() {
                break h;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(history.iter().all(|(t, d)| *t > 0.0 && *d >= 0.0));
        assert!(mon.disable_drift());
        // `strategy = None` → l'historique n'est plus exposee.
        assert!(mon.drift_history().is_none());
    }
}
