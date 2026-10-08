// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Etat partage des handlers (equivalent de ce que les endpoints
//! Python recuperent depuis `request.app`).

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use onionbit_core::{CoreSession, DaemonConfig};

/// Lignes `downloads` + `torrent_states` + `tracker_state` relues par
/// `GET /api/downloads`.
pub type DownloadsRows = (
    Vec<onionbit_db::DownloadRow>,
    Vec<onionbit_db::TorrentStateRow>,
    Vec<onionbit_db::models::TrackerStateRow>,
);

/// Fraicheur du cache `DownloadsRowsCache` — bien en dessous du tick
/// d'affichage de l'UI (1-2 s), invisible a l'ecran.
pub const DOWNLOADS_ROWS_TTL: std::time::Duration = std::time::Duration::from_millis(800);

/// Cache court **single-flight** des lignes ci-dessus : pendant la
/// restauration, chaque evenement SSE `DownloadStateChanged` redeclenche
/// un `GET /api/downloads` de l'UI (~10+/s) et chaque acces sqlite
/// prendait 300-700 ms sous contention — les requetes concurrentes se
/// coalescent sur `fetch` au lieu d'empiler des lectures identiques.
pub struct DownloadsRowsCache {
    inner: Mutex<Option<(std::time::Instant, Arc<DownloadsRows>)>>,
    fetch: tokio::sync::Mutex<()>,
}

impl DownloadsRowsCache {
    /// Retourne les lignes fraiches, en executant `fetch` une seule
    /// fois si le cache est perime (les appelants concurrents
    /// attendent puis reutilisent le resultat).
    pub async fn get_or_fetch<F>(&self, fetch: F) -> DownloadsRows
    where
        F: std::future::Future<Output = DownloadsRows>,
    {
        if let Some((fetched, rows)) = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if fetched.elapsed() < DOWNLOADS_ROWS_TTL {
                return (**rows).clone();
            }
        }
        let _guard = self.fetch.lock().await;
        // Re-test sous le verrou : un appelant precedent vient peut-etre
        // de rafraichir.
        if let Some((fetched, rows)) = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if fetched.elapsed() < DOWNLOADS_ROWS_TTL {
                return (**rows).clone();
            }
        }
        let rows = fetch.await;
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((std::time::Instant::now(), Arc::new(rows.clone())));
        rows
    }

    /// Invalide le cache (endpoints mutants de `/api/downloads`).
    pub fn invalidate(&self) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Default for DownloadsRowsCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            fetch: tokio::sync::Mutex::new(()),
        }
    }
}

/// Rate-limiter des tentatives de deverrouillage (`POST
/// /api/identity/unlock`, ADR-0016) : fenetre glissante par IP +
/// globale. Les plafonds viennent de `identity.unlock_*` (config) —
/// aucune valeur en dur ici, seule la mecanique.
pub struct UnlockRateLimiter {
    per_ip:
        Mutex<std::collections::HashMap<IpAddr, std::collections::VecDeque<std::time::Instant>>>,
    global: Mutex<std::collections::VecDeque<std::time::Instant>>,
}

impl UnlockRateLimiter {
    /// `true` si la tentative est admise (et enregistree) ; `false`
    /// si un des deux plafonds est atteint dans la fenetre.
    pub fn admit(
        &self,
        ip: IpAddr,
        per_ip_max: u32,
        global_max: u32,
        window: std::time::Duration,
    ) -> bool {
        let now = std::time::Instant::now();
        let cutoff = now.checked_sub(window).unwrap_or(now);
        let mut global = self.global.lock().unwrap_or_else(|e| e.into_inner());
        while global.front().is_some_and(|t| *t < cutoff) {
            global.pop_front();
        }
        if global.len() >= global_max as usize {
            return false;
        }
        let mut per_ip = self.per_ip.lock().unwrap_or_else(|e| e.into_inner());
        let q = per_ip.entry(ip).or_default();
        while q.front().is_some_and(|t| *t < cutoff) {
            q.pop_front();
        }
        if q.len() >= per_ip_max as usize {
            return false;
        }
        q.push_back(now);
        global.push_back(now);
        true
    }
}

impl Default for UnlockRateLimiter {
    fn default() -> Self {
        Self {
            per_ip: Mutex::new(std::collections::HashMap::new()),
            global: Mutex::new(std::collections::VecDeque::new()),
        }
    }
}

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
    /// Signal d'arret du processus notifie par `PUT /api/shutdown`
    /// (consomme par le graceful shutdown du daemon ; `None` dans les
    /// tests de l'API — la session est alors arretee sans que le
    /// processus ne quitte).
    pub shutdown_notify: Option<Arc<tokio::sync::Notify>>,
    /// Cache single-flight des lignes `downloads` + `torrent_states`
    /// lues par `GET /api/downloads` — voir `DownloadsRowsCache`.
    pub downloads_rows: Arc<DownloadsRowsCache>,
    /// Repertoire du build Flutter web servi en fallback hors `/api`
    /// (`api/web_ui_*` de `DaemonConfig` — statiques exemptes d'auth,
    /// parite `/ui`/`/static` Python ; `None` = rien de servi).
    pub web_ui_dir: Option<PathBuf>,
    /// Injecte `api_key` dans l'`index.html` servi (meta
    /// `onionbit-api-key`) pour l'auto-connexion same-origin
    /// (`api/web_ui_inject_key`, defaut `true`).
    pub web_ui_inject_key: bool,
    /// Rate-limiter de `POST /api/identity/unlock` (ADR-0016) —
    /// borne anti brute-force du mot de passe `OBSK`.
    pub unlock_limiter: Arc<UnlockRateLimiter>,
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
            shutdown_notify: None,
            downloads_rows: Arc::new(DownloadsRowsCache::default()),
            web_ui_dir: None,
            web_ui_inject_key: true,
            unlock_limiter: Arc::new(UnlockRateLimiter::default()),
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

    /// Branche le signal d'arret du daemon (`PUT /api/shutdown`
    /// terminera le processus, pas seulement la session).
    pub fn with_shutdown_notify(mut self, notify: Arc<tokio::sync::Notify>) -> Self {
        self.shutdown_notify = Some(notify);
        self
    }

    /// Sert le build Flutter web de `dir` en same-origin (statiques
    /// hors `/api`, exemptes d'authentification).
    pub fn with_web_ui_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.web_ui_dir = dir;
        self
    }

    /// Desactive l'injection de la cle API dans `index.html`
    /// (`api/web_ui_inject_key = false`).
    pub fn with_web_ui_inject_key(mut self, inject: bool) -> Self {
        self.web_ui_inject_key = inject;
        self
    }

    /// Transport furtif actif (ADR-0017) — resolu **dynamiquement**
    /// depuis la session : en demarrage differe (ADR-0016 —
    /// `identity_pending`/`locked`) la stack n'existe qu'apres
    /// resolution ; un snapshot pris au bind resterait `None` a
    /// jamais. `None` hors stealth ou avant resolution.
    pub fn stealth_transport(
        &self,
    ) -> Option<Arc<onionbit_ipv8::stealth_transport::StealthTransport>> {
        self.session
            .ipv8()
            .and_then(|stack| stack.stealth_transport.clone())
    }

    /// Pousse une erreur d'ajout dans le journal CLI (borne 100,
    /// le plus recent en tete — comme `_add_err` Python).
    pub fn push_cli_error(&self, msg: impl Into<String>) {
        let mut log = self.unhandled_cli.lock().unwrap_or_else(|e| e.into_inner());
        log.push_front(msg.into());
        log.truncate(100);
    }
}
