//! Etat partage des handlers (equivalent de ce que les endpoints
//! Python recuperent depuis `request.app`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tribler_core::{CoreSession, DaemonConfig};

/// Etat injecte dans tous les handlers.
#[derive(Clone)]
pub struct AppState {
    /// Session coeur (moteur + base + notifier).
    pub session: CoreSession,
    /// Cle API exigee par le middleware (`ApiKeyMiddleware` Python :
    /// `X-Api-Key` header, `?key=` query, cookie `api_key`). `None` ou
    /// chaine vide = pas d'authentification (idem Python quand la cle
    /// configuree est vide).
    pub api_key: Option<String>,
    /// Configuration persistee du daemon (`configuration.json`) —
    /// partagee entre `GET`/`POST /api/settings` et le daemon.
    pub daemon_config: Arc<Mutex<DaemonConfig>>,
    /// Chemin du fichier de config (reecrit apres chaque `POST
    /// /api/settings` ; `None` = pas de persistance, tests).
    pub config_path: Option<PathBuf>,
    /// File des erreurs d'ajout CLI non vues
    /// (`unhandled_cli_log` Python : insertions en tete, borne 100,
    /// videe par `GET /api/downloads/clierrors`).
    pub unhandled_cli: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// Nombre de clients SSE connectes a `/api/events` (le champ
    /// `sessions` du message `events_start` / `/api/events/info`).
    pub sse_sessions: Arc<std::sync::atomic::AtomicUsize>,
}

impl AppState {
    /// Etat par defaut autour d'une session coeur (tests : pas de
    /// cle API, pas de fichier de configuration).
    pub fn new(session: CoreSession) -> Self {
        Self {
            session,
            api_key: None,
            daemon_config: Arc::new(Mutex::new(DaemonConfig::default())),
            config_path: None,
            unhandled_cli: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            sse_sessions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Adosse l'etat a une configuration de daemon persistee dans
    /// `config_path` (active l'authentification si `api.key` est
    /// renseignee).
    pub fn with_daemon_config(self, config: DaemonConfig, config_path: Option<PathBuf>) -> Self {
        let api_key = config.api_key().map(String::from);
        Self {
            api_key,
            daemon_config: Arc::new(Mutex::new(config)),
            config_path,
            ..self
        }
    }

    /// Impose une cle API sans fichier de configuration (tests).
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Pousse une erreur d'ajout dans le journal CLI (borne 100,
    /// le plus recent en tete — comme `_add_err` Python).
    pub fn push_cli_error(&self, msg: impl Into<String>) {
        let mut log = self.unhandled_cli.lock().unwrap();
        log.push_front(msg.into());
        log.truncate(100);
    }
}
