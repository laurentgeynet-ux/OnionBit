// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `POST /api/browser/launch` — lance le navigateur (ADR-0030 §6)
//! sur une URL `.onion` (ou clearnet si `allow_clearnet_exit`
//! l'autorise) via le proxy Tor loopback.
//!
//! Garde-fous :
//!
//! - **jamais d'argv pilote par l'API** : le chemin du binaire et
//!   les arguments viennent de `tor.browser.binary_path` /
//!   `extra_args` (config seule) — le corps `{url}` ne transporte
//!   que la destination, jamais un drapeau ou une option ;
//! - **token jamais en argv** : l'eventuel token `require_auth`
//!   atteint le navigateur via le dillorc ephemere ecrit par
//!   `CoreSession::launch_browser` (jamais en ligne de commande,
//!   jamais en variable d'environnement) ;
//! - **aucune URL dans les logs** — meme regle que les termes de
//!   recherche (ADR-0028 §5).
//!
//! Erreurs typees (ADR-0030 §6) : `invalid_url` / `denied_target`
//! (400), `browser_not_configured` / `tor_disabled` /
//! `tor_not_ready` / `tor_bootstrapping` / `bootstrap_failed` (503).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use onionbit_core::BrowserLaunchError;
use onionbit_tor::TorState;

use crate::error::ApiError;
use crate::state::AppState;

use super::tor::tor_unavailable;

/// Corps de `POST /api/browser/launch` — la destination seulement
/// (`embed` est un mode de rendu, pas un drapeau argv : il se
/// traduit par `--host-shell` pose par le daemon, jamais par
/// l'appelant).
#[derive(Debug, Deserialize)]
pub struct LaunchBody {
    /// URL `http(s)://hote[:port]/chemin` ouverte dans le
    /// navigateur — validee par la politique navigateur avant
    /// tout lancement. **Vide = page d'accueil locale**
    /// (`about:onionbit`, aucune connexion, aucune cible a valider).
    pub url: String,
    /// `true` → le navigateur est lance en mode `--host-shell` et
    /// le handle de sa premiere fenetre (`hwnd`) est renvoye pour
    /// reparenting dans la fenetre de l'app. `false` (defaut) =
    /// fenetre externe classique.
    #[serde(default)]
    pub embed: bool,
}

/// `POST /api/browser/launch` — validation schema + politique +
/// `Ready`, puis spawn du binaire configure.
pub async fn browser_launch(
    State(state): State<AppState>,
    Json(body): Json<LaunchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.session.tor();
    if !runtime.settings().enabled {
        return Err(tor_unavailable(
            "tor_disabled",
            "connectivite Tor desactivee (tor.enabled=false)",
        ));
    }
    // Etat de la lane — meme filet d'auto-reparation que
    // `tor_probe` : un `Off`/`Error` retente le bootstrap plutot
    // que de figer la lane.
    match runtime.state() {
        TorState::Ready => {}
        TorState::Off | TorState::Error(_) => {
            runtime
                .bootstrap()
                .map_err(|e| tor_unavailable("bootstrap_failed", format!("bootstrap Tor : {e}")))?;
            return Err(tor_unavailable(
                "tor_bootstrapping",
                "bootstrap Tor en cours".to_string(),
            ));
        }
        TorState::Bootstrapping { progress } => {
            return Err(tor_unavailable(
                "tor_bootstrapping",
                format!("bootstrap Tor en cours ({progress}%)"),
            ));
        }
        TorState::Dormant => {
            return Err(tor_unavailable("tor_not_ready", "client Tor dormant"));
        }
    }
    let launch = state
        .session
        .launch_browser(&body.url, body.embed)
        .map_err(|e| match e {
            BrowserLaunchError::NotConfigured => tor_unavailable(
                "browser_not_configured",
                "navigateur non configure (tor.browser.binary_path, \
             tor.browser_proxy.enabled)",
            ),
            BrowserLaunchError::DeniedTarget => {
                ApiError::with_code(StatusCode::BAD_REQUEST, "denied_target", "cible refusee")
            }
            BrowserLaunchError::InvalidUrl => {
                ApiError::with_code(StatusCode::BAD_REQUEST, "invalid_url", "url invalide")
            }
            BrowserLaunchError::Io(e) => ApiError::internal(format!("lancement navigateur : {e}")),
        })?;
    Ok(Json(serde_json::json!({
        "launched": true,
        "pid": launch.pid,
        // Handle natif (HWND Win32 / XID X11) pour reparenting —
        // `null` si `embed` non demande, non supporte par le
        // navigateur ou timeout : l'app retombe en fenetre externe.
        "hwnd": launch.hwnd,
    })))
}
