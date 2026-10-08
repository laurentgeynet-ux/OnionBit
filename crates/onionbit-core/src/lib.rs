// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-core` — domaine et orchestration.
//!
//! Equivalent de `tribler.core.session` : `CoreSession` assemble les
//! ports d'infrastructure (moteur BitTorrent `onionbit-bittorrent`,
//! persistance `onionbit-db`) et publie les evenements internes sur le
//! `Notifier`. Ce crate ne connait ni HTTP ni transports — il est
//! consomme par `onionbit-api` (REST/SSE) et `onionbit-daemon`
//! (composition racine).
//!
//! Services secondaires (content_discovery, torrent_checker, rss,
//! watch_folder) : ajoutes a l'etape 14 du roadmap.

pub mod asyncio;
pub mod attestation_store;
pub mod augmenter;
pub mod config;
pub mod daemon_config;
pub mod error;
pub mod ext_ledger_store;
pub mod guard_store;
pub mod identity;
pub mod ipv8_stack;
pub mod notifier;
pub mod peer_stats_store;
pub mod queries;
pub mod services;
pub mod session;
pub mod trackers;

pub use config::CoreConfig;
pub use daemon_config::{DaemonConfig, CONFIG_FILENAME};
pub use error::{CoreError, Result};
pub use ipv8_stack::{Ipv8Config, Ipv8Stack};
pub use notifier::{Notification, Notifier};
pub use session::{CoreSession, QueueOp};

#[cfg(test)]
mod tests {
    use super::*;

    /// .torrent minimal produit par le bencode de onionbit-format.
    fn test_torrent_bytes() -> Vec<u8> {
        let mut info = std::collections::BTreeMap::new();
        info.insert(
            b"length".to_vec(),
            onionbit_format::bencode::BValue::Int(42),
        );
        info.insert(
            b"name".to_vec(),
            onionbit_format::bencode::BValue::Bytes(b"core-test.bin".to_vec()),
        );
        info.insert(
            b"piece length".to_vec(),
            onionbit_format::bencode::BValue::Int(16384),
        );
        info.insert(
            b"pieces".to_vec(),
            onionbit_format::bencode::BValue::Bytes(vec![0u8; 20]),
        );
        let mut root = std::collections::BTreeMap::new();
        root.insert(
            b"info".to_vec(),
            onionbit_format::bencode::BValue::Dict(info),
        );
        onionbit_format::bencode::encode(&onionbit_format::bencode::BValue::Dict(root))
    }

    #[tokio::test]
    async fn session_offline_ajoute_et_persiste_un_torrent() {
        let dir = tempfile::tempdir().unwrap();
        let notifier = Notifier::new();
        let mut rx = notifier.subscribe();
        let session = CoreSession::start_offline(CoreConfig::offline(dir.path().into()), notifier)
            .await
            .unwrap();

        let dl = session
            .add_torrent_bytes(test_torrent_bytes(), true)
            .await
            .unwrap();
        assert_eq!(dl.name().as_deref(), Some("core-test.bin"));
        assert_eq!(session.downloads().len(), 1);

        // Le notifier a vu l'ajout via les evenements de progression
        // (la boucle tourne en tache de fond ; on attend l'evenement
        // en sautant `SessionStarted`, emis par `try_start_identity`).
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if matches!(
                    rx.recv().await.unwrap(),
                    Notification::DownloadProgress(_)
                ) {
                    return;
                }
            }
        })
        .await
        .expect("aucune notification de progression recue");

        session.stop().await;
    }

    /// Un add magnet droppé en pleine résolution BEP 9 (timeout ou
    /// annulation côté appelant) ne doit pas laisser l'entrée
    /// `pending` orpheline — sinon tout re-add échouait en
    /// `InvalidState("déjà en cours de résolution")` à vie.
    #[tokio::test]
    async fn add_magnet_abandonne_nettoie_pending() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = CoreConfig::offline(dir.path().into());
        // Un tracker injoignable garde le flux de pairs ouvert
        // (`TrackerComms` retente a l'infini) : la resolution BEP 9
        // attend au lieu d'echouer immediatement quand le pair local
        // est rejete par le kernel (RST immediat sur Linux/macOS).
        cfg.engine.disable_trackers = false;
        let session = CoreSession::start_offline(cfg, Notifier::new())
            .await
            .unwrap();
        let ih = "a".repeat(40);
        let uri = format!("magnet:?xt=urn:btih:{ih}&dn=ghost.bin&tr=udp%3A%2F%2F127.0.0.1%3A9");
        let s = session.clone();
        let task =
            tokio::spawn(async move { s.add_download_anon(&uri, false, 0, false, None).await });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !session.is_pending(&ih) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "entree pending jamais creee"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        task.abort();
        let _ = task.await;
        assert!(
            !session.is_pending(&ih),
            "entree pending orpheline apres abandon de l'add"
        );
        session.stop().await;
    }

    #[test]
    fn notifier_sans_abonne_ne_bloque_pas() {
        let n = Notifier::new();
        n.notify(Notification::SessionStarted);
        // Pas de panique ni de blocage sans abonnes.
        let mut rx = n.subscribe();
        n.notify(Notification::SessionStopping);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Notification::SessionStopping
        ));
    }
}
