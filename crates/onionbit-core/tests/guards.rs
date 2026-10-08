// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cycle de vie complet de la persistance des guard nodes
//! (ADR-0010) a travers `CoreSession` : une session adopte un guard,
//! s'arrete, et la session suivante sur le meme `state_dir` recharge
//! le set depuis `onionbit.db` — le guard adopte n'est pas perdu au
//! redemarrage du daemon (sinon la valeur anti-Sybil des guards
//! s'evanouirait a chaque relance).
//!
//! Hors-ligne : endpoint IPv8 loopback ephemere, aucun pair distant.

use std::net::SocketAddr;
use std::sync::Arc;

use onionbit_core::config::CoreConfig;
use onionbit_core::notifier::Notifier;
use onionbit_core::session::CoreSession;
use onionbit_ipv8::{Peer, UdpAddress};

/// Demarre une session fichier (DB `onionbit.db` persistee) avec la
/// stack IPv8 + tunnel + guards actives.
async fn start_guarded(dir: &std::path::Path) -> Arc<CoreSession> {
    let mut cfg = CoreConfig::offline(dir.to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.guards_enabled = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    Arc::new(
        CoreSession::start(cfg, Notifier::new())
            .await
            .expect("start session"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guards_survivent_au_redemarrage_du_daemon() {
    let dir = tempfile::tempdir().unwrap();

    // Premiere session : adopte un guard (pool d'un seul candidat —
    // l'adoption est deterministe).
    let session = start_guarded(dir.path()).await;
    let tunnel = session
        .ipv8()
        .expect("stack ipv8")
        .tunnel
        .clone()
        .expect("tunnel community");
    let sk = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
    let peer = Peer::new(
        sk.public_key().to_bin(),
        Some(UdpAddress::from(
            "10.20.30.40:7759".parse::<SocketAddr>().unwrap(),
        )),
    )
    .unwrap();
    tunnel.guards.ensure(std::slice::from_ref(&peer));
    assert_eq!(
        tunnel.guards.guard_keys(),
        vec![peer.public_key_bin.clone()],
        "guard adopte dans la premiere session"
    );
    session.stop().await;

    // Deuxieme session sur le meme state_dir : la table `guards`
    // persistante doit repeupler le set (`attach_store` au demarrage
    // du tunnel, injecte seulement quand `guards_enabled`).
    let session2 = start_guarded(dir.path()).await;
    let tunnel2 = session2
        .ipv8()
        .expect("stack ipv8")
        .tunnel
        .clone()
        .expect("tunnel community");
    assert_eq!(
        tunnel2.guards.guard_keys(),
        vec![peer.public_key_bin.clone()],
        "guard recharge depuis onionbit.db au redemarrage"
    );
    session2.stop().await;
}

/// Contrepartie : avec `guards_enabled = false`, rien n'est injecte
/// ni charge — le set reste vide meme si la base contient des
/// enregistrements (repli pyipv8 exact, ADR-0010).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guards_desactivees_ne_chargent_pas_le_set() {
    let dir = tempfile::tempdir().unwrap();

    let session = start_guarded(dir.path()).await;
    let tunnel = session
        .ipv8()
        .expect("stack ipv8")
        .tunnel
        .clone()
        .expect("tunnel community");
    let sk = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
    let peer = Peer::new(
        sk.public_key().to_bin(),
        Some(UdpAddress::from(
            "10.20.30.41:7760".parse::<SocketAddr>().unwrap(),
        )),
    )
    .unwrap();
    tunnel.guards.ensure(std::slice::from_ref(&peer));
    session.stop().await;

    // Redemarrage SANS la feature : le store n'est pas injecte,
    // le set reste vide.
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.guards_enabled = false;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    let session2 = CoreSession::start(cfg, Notifier::new()).await.unwrap();
    let tunnel2 = session2
        .ipv8()
        .expect("stack ipv8")
        .tunnel
        .clone()
        .expect("tunnel community");
    assert!(
        tunnel2.guards.guard_keys().is_empty(),
        "set vide sans la feature"
    );
    session2.stop().await;
}
