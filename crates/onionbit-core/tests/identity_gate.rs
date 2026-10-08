// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Gate d'identite ADR-0016 (etape 48d) : demarrage differe en
//! `identity_pending`/`locked`, resolutions (create / restore /
//! invite / unlock), idempotence de `try_start_identity`, et
//! garantie « aucun composant identitaire n'existe avant la
//! resolution » (pas de socket, pas de fichier, rien a signer).

use onionbit_core::identity::{self, IdentityMaterial};
use onionbit_core::session::IdentityPhase;
use onionbit_core::{CoreConfig, CoreSession, Notifier};

/// Session avec stack IPv8 active (loopback, ports ephemeres) —
/// `stack_enabled` est la condition du gate.
fn gated_cfg(state_dir: std::path::PathBuf) -> CoreConfig {
    let mut cfg = CoreConfig::offline(state_dir);
    cfg.ipv8.enabled = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg
}

fn no_identity_files(dir: &std::path::Path) -> bool {
    // ADR-0018 etape 58 : fichiers identitaires sous `state/identity/`.
    let dir = dir.join("identity");
    !dir.join(identity::IDENTITY_SEED_FILE).exists()
        && !dir.join(identity::IPV8_KEY_FILE).exists()
        && !dir.join(identity::STEALTH_BRIDGE_KEY_FILE).exists()
}

/// Premier boot sous gate : la session expose l'API en `pending`
/// sans identite, sans moteur, sans stack — rien ne peut signer.
#[tokio::test]
async fn pending_shell_sans_identite_ni_composants() {
    let dir = tempfile::tempdir().unwrap();
    let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), true)
        .await
        .expect("shell pending");
    assert_eq!(session.identity_phase(), IdentityPhase::Pending);
    assert!(session.ipv8().is_none(), "aucune stack avant resolution");
    assert!(session.engine().is_none(), "aucun moteur avant resolution");
    assert!(no_identity_files(dir.path()), "aucun fichier identite");
    session.stop().await;
}

/// Headless (sans gate) : auto-generation historique preservee.
#[tokio::test]
async fn headless_sans_gate_auto_genere() {
    let dir = tempfile::tempdir().unwrap();
    let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), false)
        .await
        .expect("boot headless");
    assert_eq!(session.identity_phase(), IdentityPhase::Ready);
    assert!(session.ipv8().is_some());
    assert!(dir
        .path()
        .join("identity")
        .join(identity::IDENTITY_SEED_FILE)
        .exists());
    session.stop().await;
}

/// Resolution « invite » : phase `Ready`, identite ephemere, aucun
/// fichier ecrit — et deux boots invites donnent deux cles
/// publiques distinctes.
#[tokio::test]
async fn invite_resout_sans_persistance() {
    let mut pks = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), true)
            .await
            .expect("shell pending");
        session
            .try_start_identity(Some(IdentityMaterial::guest()))
            .await
            .expect("guest start");
        assert_eq!(session.identity_phase(), IdentityPhase::Ready);
        assert!(session.is_guest());
        let stack = session.ipv8().expect("stack apres resolution");
        pks.push(stack.public_key_hex());
        session.stop().await;
        assert!(
            no_identity_files(dir.path()),
            "une session invitee n'ecrit aucun artefact identite"
        );
    }
    assert_ne!(pks[0], pks[1], "deux sessions invitees = deux identites");
}

/// Resolution « nouvelle identite » en pending : fichiers crees,
/// session prete — puis `try_start_identity` est idempotent.
#[tokio::test]
async fn pending_create_puis_double_demarrage_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), true)
        .await
        .expect("shell pending");
    let material = identity::create_seed(dir.path(), None).expect("create");
    session
        .try_start_identity(Some(material))
        .await
        .expect("identity start");
    assert_eq!(session.identity_phase(), IdentityPhase::Ready);
    assert!(session.ipv8().is_some());
    // Double resolution : Ok, rien n'est redemarre.
    session
        .try_start_identity(Some(IdentityMaterial::guest()))
        .await
        .expect("idempotent");
    assert!(!session.is_guest(), "le second materiel est ignore");
    session.stop().await;
}

/// `identity.at_rest` : le boot tombe en `locked`, le mauvais mot
/// de passe echoue uniformement, le bon demarre la session avec la
/// meme identite — sans jamais ecrire de cache clair.
#[tokio::test]
async fn locked_puis_unlock_demarre() {
    let dir = tempfile::tempdir().unwrap();
    // Premier boot : identite seedee puis scellee (pas de gate —
    // l'auto-generation pose la graine).
    let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), false)
        .await
        .expect("boot initial");
    let pk = session.ipv8().expect("stack").public_key_hex();
    identity::seal_seed(dir.path(), b"pw de test").expect("seal");
    session.stop().await;

    // Reboot : `locked`, aucun composant reseau.
    let session = CoreSession::start_gated(gated_cfg(dir.path().into()), Notifier::new(), false)
        .await
        .expect("shell locked");
    assert_eq!(session.identity_phase(), IdentityPhase::Locked);
    assert!(session.ipv8().is_none() && session.engine().is_none());
    // Mauvais mot de passe : refus.
    assert!(identity::unlock_seed(dir.path(), b"faux").is_err());
    // Bon : materiel derive en memoire, session complete.
    let material = identity::unlock_seed(dir.path(), b"pw de test").expect("unlock");
    session
        .try_start_identity(Some(material))
        .await
        .expect("identity start");
    assert_eq!(session.identity_phase(), IdentityPhase::Ready);
    assert_eq!(session.ipv8().expect("stack").public_key_hex(), pk);
    // La graine reste scellee ; aucun cache clair n'est reecrit.
    assert!(!dir.path().join(identity::IPV8_KEY_FILE).exists());
    session.stop().await;
}

/// `at_rest` × `stealth.role != client` : refus ferme a la session.
#[tokio::test]
async fn at_rest_x_role_serveur_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = gated_cfg(dir.path().into());
    cfg.identity_at_rest = true;
    cfg.ipv8.enabled = false;
    cfg.ipv8.stealth = Some(onionbit_core::daemon_config::StealthFileConfig {
        enabled: true,
        role: "bridge".into(),
        ..Default::default()
    });
    let r = CoreSession::start_gated(cfg, Notifier::new(), false).await;
    assert!(r.is_err(), "at_rest + pont doit etre refuse ferme");
}
