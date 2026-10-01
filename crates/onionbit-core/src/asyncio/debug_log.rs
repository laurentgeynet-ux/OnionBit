// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `DequeLogHandler` Python (`asyncio_endpoint.py`) adapte a
//! `tracing` : buffer borne de messages alimente par une couche
//! [`DebugLogLayer`] gatee par `enable`, plus rechargement a chaud
//! du `EnvFilter` global du daemon (le filtre `info` par defaut
//! empecherait les evenements `debug` d'atteindre la couche).

use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// `maxlen=50` du `deque` Python.
const DEBUG_LOG_CAPACITY: usize = 50;

/// `loop.slow_callback_duration` par defaut d'asyncio (0,1 s).
pub const DEFAULT_SLOW_CALLBACK_DURATION: f64 = 0.1;

/// Buffer borne de messages de debug (`self.deque` du
/// `DequeLogHandler` — formattes `%(message)s`).
pub struct DebugLogBuffer {
    /// `asyncio_log_handler is not None` Python.
    enabled: AtomicBool,
    /// `loop.slow_callback_duration` (secondes, float).
    slow_callback_duration: Mutex<f64>,
    messages: Mutex<VecDeque<String>>,
}

static BUFFER: DebugLogBuffer = DebugLogBuffer::new();

/// Le buffer global du daemon (partage entre la couche tracing et
/// l'endpoint `/api/ipv8/asyncio/debug`).
pub fn debug_log() -> &'static DebugLogBuffer {
    &BUFFER
}

impl DebugLogBuffer {
    const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            slow_callback_duration: Mutex::new(DEFAULT_SLOW_CALLBACK_DURATION),
            messages: Mutex::new(VecDeque::new()),
        }
    }

    /// `loop.set_debug(enable)` + ajout/retrait du handler : gate
    /// la capture et recharge le filtre global (`debug` / filtre
    /// d'origine) via le hook installe par `init_tracing`.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        apply_debug_filter(enabled);
    }

    /// `loop.get_debug()`.
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// `emit` : empile un message formate (borne [`DEBUG_LOG_CAPACITY`]).
    pub fn push(&self, message: String) {
        let mut messages = self.messages.lock().unwrap();
        if messages.len() >= DEBUG_LOG_CAPACITY {
            messages.pop_front();
        }
        messages.push_back(message);
    }

    /// Snapshot des messages pour `GET /debug`.
    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().iter().cloned().collect()
    }

    /// `loop.slow_callback_duration`.
    pub fn slow_callback_duration(&self) -> f64 {
        *self.slow_callback_duration.lock().unwrap()
    }

    /// `loop.slow_callback_duration = ...`.
    pub fn set_slow_callback_duration(&self, value: f64) {
        *self.slow_callback_duration.lock().unwrap() = value;
    }
}

/// Hook de rechargement du `EnvFilter` global : `true` → niveau
/// `debug`, `false` → filtre d'origine (`RUST_LOG` ou `info`).
/// Installe par `init_tracing` du daemon ; absent en tests.
static FILTER_RELOAD: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();

/// `init_tracing` enregistre son `reload::Handle` ici — la couche
/// REST ne depend pas de `onionbit-daemon`.
pub fn set_filter_reload<F>(reload: F)
where
    F: Fn(bool) + Send + Sync + 'static,
{
    let _ = FILTER_RELOAD.set(Box::new(reload));
}

/// Recharge le filtre global si un hook est installe.
fn apply_debug_filter(enabled: bool) {
    if let Some(reload) = FILTER_RELOAD.get() {
        reload(enabled);
    }
}

/// Visiteur qui extrait le champ `message` d'un evenement
/// (equivalent du `Formatter` par defaut = `%(message)s`).
struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// Couche `tracing_subscriber` alimentant [`DebugLogBuffer`].
///
/// A installer inconditionnellement dans `init_tracing` — le gate
/// `enabled` rend la capture nulle tant que le debug est desactive
/// (equivalent du handler ajoute/retire a chaud en Python).
pub struct DebugLogLayer;

impl<S: Subscriber> Layer<S> for DebugLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let buffer = debug_log();
        if !buffer.enabled() {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if !visitor.0.is_empty() {
            buffer.push(visitor.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_borne_et_flags() {
        let buffer = DebugLogBuffer::new();
        assert!(!buffer.enabled());
        buffer.set_enabled(true);
        assert!(buffer.enabled());
        for i in 0..60 {
            buffer.push(format!("m{i}"));
        }
        let messages = buffer.messages();
        assert_eq!(messages.len(), DEBUG_LOG_CAPACITY);
        assert_eq!(messages.first().map(String::as_str), Some("m10"));
        buffer.set_slow_callback_duration(0.5);
        assert_eq!(buffer.slow_callback_duration(), 0.5);
    }
}
