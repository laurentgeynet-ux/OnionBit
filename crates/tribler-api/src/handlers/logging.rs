//! Handler `/api/logging` — equivalent de
//! `tribler.core.restapi.logging_endpoint`.
//!
//! Python sert le `memory_logger` (buffer circulaire). Ici le daemon
//! journalise vers stdout/`tracing` — on lit la fin du fichier de log
//! `state_dir/logs/*.log` s'il existe, sinon une reponse vide
//! documentee.

use axum::extract::{Query, State};
use serde::Deserialize;

use crate::state::AppState;

/// `GET /api/logging?max_lines=N` — dernieres lignes de log.
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// Nombre de lignes max (defaut 200).
    pub max_lines: Option<usize>,
}

/// Taille max du segment lu en fin de fichier (1 Mio).
const READ_TAIL: u64 = 1_048_576;

pub async fn get_logs(State(state): State<AppState>, Query(q): Query<LogsQuery>) -> String {
    let max_lines = q.max_lines.unwrap_or(200);
    let logs_dir = state.session.config().state_dir.join("logs");
    let mut candidates: Vec<_> = std::fs::read_dir(&logs_dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    // Le plus recent.
    candidates.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
            .unwrap_or(0)
    });
    let Some(path) = candidates.pop() else {
        return String::new();
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return String::new();
    };
    use std::io::{Read, Seek, SeekFrom};
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut file = file;
    let _ = file.seek(SeekFrom::End(-(READ_TAIL.min(len) as i64)));
    let mut buf = Vec::new();
    if file.take(READ_TAIL).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(max_lines);
    lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n")
}
