//! `/api/ipv8/asyncio/*` — equivalent de
//! `pyipv8/ipv8/REST/asyncio_endpoint.py` adapte au runtime tokio :
//!
//! - `GET /drift` — `DriftMeasurementStrategy.history` (404 `Core
//!   drift disabled.` tant que la mesure n'est pas activee).
//! - `PUT /drift` — `{"enable": bool}` : lance/arrete la mesure
//!   (`Session not initialized.` sans stack IPv8).
//! - `GET /tasks` — registre des taches nommees (tokio n'a pas
//!   d'`all_tasks()`).
//! - `PUT /debug` — `{"enable", "slow_callback_duration"}` : capture
//!   `tracing` vers un buffer borne + rechargement du `EnvFilter`.
//! - `GET /debug` — messages captures + etat du mode debug.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

/// Shape `{"error": {"handled": false, "message": ...}}` en 500 — les
/// exceptions non gerees de `base_endpoint.py`.
fn unhandled(message: String) -> Response {
    crate::error::ApiError::internal(message).into_response()
}

/// `await request.json()` Python : corps non-JSON → exception →
/// 500 non geree.
fn parse_json_body(body: &Bytes) -> Result<Value, Box<Response>> {
    serde_json::from_slice(body).map_err(|e| Box::new(unhandled(format!("JSONDecodeError: {e}"))))
}

/// Verite Python (`if parameters.get("enable")`) : null/false/0/""/
/// collections vides → `false`, le reste → `true`.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `retrieve_drift` : `{"measurements": [{timestamp, drift}]}` ou
/// 404 `{"success": false, "error": "Core drift disabled."}`.
pub async fn get_drift(State(state): State<AppState>) -> Response {
    match state.session.asyncio().drift_history() {
        Some(history) => Json(json!({
            "measurements": history
                .iter()
                .map(|(t, d)| json!({"timestamp": t, "drift": d}))
                .collect::<Vec<_>>()
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "Core drift disabled."})),
        )
            .into_response(),
    }
}

/// `enable_measurements` : `PUT /drift` — `{"enable": bool}`.
///
/// Corps invalide → 500 ; `enable` absent → 400
/// `{"error": "incorrect parameters"}` (sans `success`, fidèle) ;
/// session IPv8 absente → `{"success": false, "error": "Session not
/// initialized."}` (code 200 comme Python).
pub async fn set_drift(State(state): State<AppState>, body: Bytes) -> Response {
    let parameters = match parse_json_body(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(enable) = parameters.get("enable") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "incorrect parameters"})),
        )
            .into_response();
    };
    let enable = truthy(enable);
    // `enable()`/`disable()` echouent sans `self.session` (IPv8).
    if state.session.ipv8().is_none() {
        return Json(json!({"success": false, "error": "Session not initialized."}))
            .into_response();
    }
    let monitor = state.session.asyncio();
    let status = if enable {
        monitor.enable_drift()
    } else {
        monitor.disable_drift()
    };
    if status {
        Json(json!({"success": true})).into_response()
    } else {
        Json(json!({"success": false, "error": "Session not initialized."})).into_response()
    }
}

/// `get_asyncio_tasks` : `{"tasks": [...]}` — les taches nommees du
/// daemon (tokio n'introspecte pas ; `running`/`stack` n'ont pas
/// d'equivalent → `false`/`[]`, cf. ADR-0006).
pub async fn get_tasks(State(state): State<AppState>) -> Response {
    let tasks = state
        .session
        .asyncio()
        .tasks
        .snapshot()
        .iter()
        .map(|t| {
            let mut d = json!({
                "name": t.name,
                "running": false,
                "stack": Vec::<String>::new(),
            });
            // Attributs specifiques aux taches `TaskManager` Python.
            if let Some(tm) = &t.taskmanager {
                d["taskmanager"] = json!(tm);
                d["start_time"] = json!(t.start_time);
                if let Some(interval) = t.interval {
                    d["interval"] = json!(interval);
                }
            }
            d
        })
        .collect::<Vec<_>>();
    Json(json!({"tasks": tasks})).into_response()
}

/// `set_asyncio_debug` : `PUT /debug` — `{"enable"?, "slow_callback_duration"?}`.
/// Aucun des deux → 400 `{"success": false, ...}` ; sinon applique et
/// `{"success": true}`.
pub async fn set_debug(State(state): State<AppState>, body: Bytes) -> Response {
    let parameters = match parse_json_body(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if parameters.get("enable").is_none() && parameters.get("slow_callback_duration").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "incorrect parameters"})),
        )
            .into_response();
    }
    let buffer = tribler_core::asyncio::debug_log();
    if let Some(d) = parameters.get("slow_callback_duration") {
        buffer.set_slow_callback_duration(d.as_f64().unwrap_or_default());
    }
    if let Some(enable) = parameters.get("enable") {
        let enable = truthy(enable);
        buffer.set_enabled(enable);
        // Persiste le choix (`logging/debug`) : `init_tracing` le
        // restaure au redemarrage — le basculement info/debug n'est
        // plus un etat perdu a chaque lancement.
        let mut cfg = state.daemon_config.lock().unwrap();
        cfg.logging.debug = enable;
        if let Some(path) = &state.config_path {
            if let Err(e) = cfg.write(path) {
                return unhandled(format!("ecriture configuration.json: {e}"));
            }
        }
    }
    Json(json!({"success": true})).into_response()
}

/// `get_asyncio_debug` : `{"messages": [{message}], "enable",
/// "slow_callback_duration"}`.
pub async fn get_debug(State(_state): State<AppState>) -> Response {
    let buffer = tribler_core::asyncio::debug_log();
    Json(json!({
        "messages": buffer
            .messages()
            .iter()
            .map(|m| json!({"message": m}))
            .collect::<Vec<_>>(),
        "enable": buffer.enabled(),
        "slow_callback_duration": buffer.slow_callback_duration(),
    }))
    .into_response()
}
