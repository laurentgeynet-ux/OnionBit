// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/stealth` (ADR-0017, etape 52) — extension propre au
//! portage, aucun equivalent Tribler.
//!
//! **Ne jamais exposer** : cle publique ou privee, secret, lien
//! `onionbit-bridge://` complet ni adresse de pont — la surface ne
//! rapporte que des compteurs et des etats. Toutes les routes vivent
//! sous le routeur `/api` → `api_key_auth` s'applique deja.

use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::Json;

use onionbit_ipv8::stealth_transport::BridgeEntry;
use onionbit_ipv8::DatagramTransport;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/stealth` — etat du mode furtif : role, presence du
/// transport, nombre de sessions et compteurs internes bornes.
/// Aucune adresse, cle ou secret dans la reponse.
pub async fn get_stealth(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let enabled = cfg.stealth.enabled;
    let role = cfg.stealth.role.clone();
    let bridges_configured = cfg.stealth.bridges.len();
    let cover = cfg.stealth.cover_traffic;
    drop(cfg);

    // Resolution dynamique (ADR-0016) : le transport n'existe qu'une
    // fois l'identite demarree — jamais fige au bind de l'API.
    let t = state.stealth_transport();
    let t = t.as_ref();
    let metrics = t.map(|t| {
        let m = t.metrics();
        serde_json::json!({
            "hs1_sent": m.hs1_sent.load(Ordering::Relaxed),
            "hs1_accepted": m.hs1_accepted.load(Ordering::Relaxed),
            "hs1_rejected": m.hs1_rejected.load(Ordering::Relaxed),
            "hs2_accepted": m.hs2_accepted.load(Ordering::Relaxed),
            "hs2_rejected": m.hs2_rejected.load(Ordering::Relaxed),
            "rl_ip_dropped": m.rl_ip_dropped.load(Ordering::Relaxed),
            "rl_global_dropped": m.rl_global_dropped.load(Ordering::Relaxed),
            "rx_garbage": m.rx_garbage.load(Ordering::Relaxed),
            "frames_in": m.frames_in.load(Ordering::Relaxed),
            "frames_out": m.frames_out.load(Ordering::Relaxed),
            "frames_rejected": m.frames_rejected.load(Ordering::Relaxed),
            "queue_dropped": m.queue_dropped.load(Ordering::Relaxed),
            "sessions_expired": m.sessions_expired.load(Ordering::Relaxed),
            "cover_sent": m.cover_sent.load(Ordering::Relaxed),
        })
    });
    let (bytes_up, bytes_down) = t.map(|t| t.bytes_counters()).unwrap_or((0, 0));

    Json(serde_json::json!({
        "enabled": enabled,
        "role": role,
        "transport_active": t.is_some(),
        "sessions": t.map(|t| t.session_count()).unwrap_or(0),
        "bridges_configured": bridges_configured,
        "cover_traffic": cover,
        "bytes_up": bytes_up,
        "bytes_down": bytes_down,
        "metrics": metrics,
    }))
}

/// `POST /api/stealth/bridges` — ajout a chaud d'un pont via son
/// lien d'invitation `{"bridge": "onionbit-bridge://<ip>:<port>#<pk>"}`.
/// Persiste dans `configuration.json` et, si le transport stealth
/// tourne, injecte l'entree sans redemarrage.
pub async fn add_bridge(
    State(state): State<AppState>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let link = req
        .get("bridge")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"bridge\" link"))?;
    let entry = BridgeEntry::parse_link(link)
        .map_err(|e| ApiError::bad_request(format!("lien bridge invalide: {e}")))?;

    let mut cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Dedup sur la chaine exacte ; une meme adresse avec une autre
    // cle = nouvelle entree (re-emission d'invitation).
    if !cfg.stealth.bridges.iter().any(|b| b == link) {
        cfg.stealth.bridges.push(link.to_string());
    }
    let total = cfg.stealth.bridges.len();
    if let Some(path) = &state.config_path {
        cfg.write(path)
            .map_err(|e| ApiError::internal(format!("ecriture configuration.json: {e}")))?;
    }
    drop(cfg);

    if let Some(t) = state.stealth_transport() {
        t.add_bridge(entry);
    }
    state
        .session
        .notifier()
        .notify(onionbit_core::Notification::SettingsChanged);

    Ok(Json(serde_json::json!({
        "modified": true,
        "bridges": total,
    })))
}
