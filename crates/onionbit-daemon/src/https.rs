// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listener HTTPS de l'API de controle (`api/https_*` Python —
//! `start_https_site` de `rest_manager.py`) : un second site sert le
//! meme routeur axum en TLS, `https_port_running` est reecrit avec le
//! port reellement lie.
//!
//! Certificat : `https_certfile` contient certificat + cle privee dans
//! le meme fichier PEM (comme `SSLContext.load_cert_chain` sans
//! keyfile). Un fichier absent ou invalide est remplace par un
//! certificat auto-signe `rcgen` (localhost) — ecrit a l'emplacement
//! configure pour etre reutilise aux prochains demarrages (Python
//! echoue au `load_cert_chain` dans ce cas ; l'auto-generation est un
//! ecart assumé, documente dans `docs/reference_tribler/`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum_server::tls_rustls::RustlsConfig;

/// Grace period de l'arret TLS (idem `shutdown_timeout` aiohttp) —
/// invoquee par le main sur le `Handle` retourne par [`spawn`].
pub const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum HttpsError {
    /// Adresse d'ecoute invalide.
    #[error("adresse https invalide {0}")]
    Addr(String),
    /// `https_host` non loopback — meme politique que l'ecoute HTTP.
    #[error("l'ecoute HTTPS doit etre loopback (127.0.0.1 ou ::1) : {0}")]
    NotLoopback(SocketAddr),
    /// Lecture/ecriture du fichier PEM.
    #[error("certificat https_certfile : {0}")]
    Io(#[from] std::io::Error),
    /// PEM sans certificat ni cle privée.
    #[error("https_certfile : {0}")]
    Pem(String),
    /// Echec de la generation `rcgen`.
    #[error("generation du certificat : {0}")]
    Cert(String),
    /// Echec du serveur TLS.
    #[error("serveur https : {0}")]
    Serve(String),
}

/// Demarre le listener HTTPS sur `https_host:https_port` et retourne
/// `(port reellement lie, handle d'arret)` — le `Handle` permet au
/// main d'appeler `graceful_shutdown` dans sa sequence d'arret
/// (un second wait sur `ShutdownSignal` volerait le `notify_one`
/// unique au detriment de l'arret principal).
///
/// `certfile` relatif est resolu contre `state_dir` (le
/// `TriblerConfigManager` Python resout les chemins relatifs contre
/// le dossier d'etat).
pub async fn spawn(
    app: Router,
    host: &str,
    port: u16,
    certfile: &str,
    state_dir: &Path,
) -> Result<(u16, axum_server::Handle<SocketAddr>), HttpsError> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|_| HttpsError::Addr(format!("{host}:{port}")))?;
    if !addr.ip().is_loopback() {
        return Err(HttpsError::NotLoopback(addr));
    }
    let cert_path = resolve(certfile, state_dir);
    let tls = load_or_generate(&cert_path).await?;

    // Bind manuel pour connaitre le port reel (0 = ephemere).
    let std_listener = std::net::TcpListener::bind(addr)?;
    // `tokio::net::TcpListener::from_std` (appele par `from_tcp`)
    // exige un socket non-bloquant — sinon l'accept loop n'est
    // jamais reveille et le handshake TLS reste fige.
    std_listener.set_nonblocking(true)?;
    let bound = std_listener.local_addr()?.port();

    let handle = axum_server::Handle::new();
    tokio::spawn({
        let handle = handle.clone();
        async move {
            let serve = axum_server::from_tcp_rustls(std_listener, tls)
                .map_err(|e| HttpsError::Serve(e.to_string()));
            match serve {
                Ok(server) => {
                    if let Err(e) = server.handle(handle).serve(app.into_make_service()).await {
                        tracing::error!(error = %e, "serveur HTTPS en erreur");
                    }
                }
                Err(e) => tracing::error!(error = %e, "serveur HTTPS impossible"),
            }
        }
    });
    tracing::info!(%addr, "API HTTPS demarree");
    Ok((bound, handle))
}

/// Chemin du PEM : spec `@root/…` resolu contre les racines
/// portables (ADR-0018), absolu tel quel, relatif resolu contre
/// `state_dir` (resolution `TriblerConfigManager` Python).
fn resolve(certfile: &str, state_dir: &Path) -> PathBuf {
    let p = onionbit_core::paths::PathRoots::for_state_dir(state_dir).resolve_persisted(certfile);
    if p.is_absolute() {
        p
    } else {
        state_dir.join(p)
    }
}

/// Charge le PEM `https_certfile` (cert + cle dans le meme fichier) ;
/// absent ou sans materiel utilisable -> certificat auto-signe `rcgen`
/// localhost, ecrit au meme chemin pour persister.
async fn load_or_generate(path: &Path) -> Result<RustlsConfig, HttpsError> {
    if let Ok(bytes) = std::fs::read(path) {
        match rustls_config_from_pem(&bytes).await {
            Ok(cfg) => return Ok(cfg),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "https_certfile invalide, regeneration");
            }
        }
    }
    let (cert_pem, key_pem) = generate_self_signed()?;
    std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
    std::fs::write(path, format!("{cert_pem}{key_pem}"))?;
    tracing::info!(path = %path.display(), "certificat https auto-signe genere");
    RustlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes())
        .await
        .map_err(|e| HttpsError::Pem(e.to_string()))
}

/// Parse le PEM complet : tous les blocs `CERTIFICATE` en chaine,
/// premiere cle privee (PKCS8/RSA/SEC1 — meme permissivite que
/// `load_cert_chain`).
async fn rustls_config_from_pem(bytes: &[u8]) -> Result<RustlsConfig, HttpsError> {
    let mut certs = Vec::new();
    let mut key = None;
    for item in rustls_pemfile::read_all(&mut &bytes[..]) {
        match item.map_err(|e| HttpsError::Pem(e.to_string()))? {
            rustls_pemfile::Item::X509Certificate(c) => certs.push(c.to_vec()),
            rustls_pemfile::Item::Pkcs1Key(k) => key = Some(k.secret_pkcs1_der().to_vec()),
            rustls_pemfile::Item::Pkcs8Key(k) => key = Some(k.secret_pkcs8_der().to_vec()),
            rustls_pemfile::Item::Sec1Key(k) => key = Some(k.secret_sec1_der().to_vec()),
            _ => {}
        }
    }
    if certs.is_empty() {
        return Err(HttpsError::Pem("aucun certificat".into()));
    }
    let Some(key) = key else {
        return Err(HttpsError::Pem("aucune cle privee".into()));
    };
    RustlsConfig::from_der(certs, key)
        .await
        .map_err(|e| HttpsError::Pem(e.to_string()))
}

/// Certificat auto-signe `localhost` (SAN `localhost` + `127.0.0.1`
/// + `::1` — l'API HTTPS reste loopback).
fn generate_self_signed() -> Result<(String, String), HttpsError> {
    let certified = rcgen::generate_simple_self_signed(vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
    ])
    .map_err(|e| HttpsError::Cert(e.to_string()))?;
    Ok((certified.cert.pem(), certified.key_pair.serialize_pem()))
}
