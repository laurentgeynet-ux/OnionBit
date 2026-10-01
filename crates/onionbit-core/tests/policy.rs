// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Anti-SSRF de `Session::add_download` : une URI `http(s)` ne doit
//! resolver que vers des adresses autorisees par `ip_policy`.
//!
//! Hors-ligne : les hotes testes sont numeriques (pas de DNS) ou
//! resolvent en loopback.

use onionbit_core::config::CoreConfig;
use onionbit_core::error::CoreError;
use onionbit_core::notifier::Notifier;
use onionbit_core::session::CoreSession;

/// Politique stricte : loopback refuse -> l'ajout echoue AVANT tout
/// appel reseau (la destination numerique est rejetee par la
/// politique, aucun trafic n'est emis).
#[tokio::test]
async fn http_uri_to_loopback_denied_by_strict_policy() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ip_policy = onionbit_network_policy::IpPolicy::strict();
    let session = CoreSession::start_offline(cfg, Notifier::new())
        .await
        .unwrap();
    let res = session
        .add_download("http://127.0.0.1:9/file.torrent", false)
        .await;
    assert!(
        matches!(res, Err(CoreError::Policy(_))),
        "URI loopback acceptee en politique stricte : {res:?}"
    );
    session.stop().await;
}

/// Politique permissive : l'URI loopback passe le garde-fou (l'ajout
/// echouera ensuite cote moteur, faute de serveur — ce n'est plus un
/// refus de politique).
#[tokio::test]
async fn http_uri_to_loopback_allowed_by_permissive_policy() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().to_path_buf());
    let session = CoreSession::start_offline(cfg, Notifier::new())
        .await
        .unwrap();
    let res = session
        .add_download("http://127.0.0.1:9/file.torrent", false)
        .await;
    assert!(
        !matches!(res, Err(CoreError::Policy(_))),
        "URI loopback refusee en politique permissive : {res:?}"
    );
    session.stop().await;
}
