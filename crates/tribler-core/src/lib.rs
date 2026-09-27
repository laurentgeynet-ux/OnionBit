//! `tribler-core` — domaine et orchestration.
//!
//! Equivalent de `tribler.core.session` : `CoreSession` assemble les
//! ports d'infrastructure (moteur BitTorrent `tribler-bittorrent`,
//! persistance `tribler-db`) et publie les evenements internes sur le
//! `Notifier`. Ce crate ne connait ni HTTP ni transports — il est
//! consomme par `tribler-api` (REST/WebSocket) et `tribler-daemon`
//! (composition racine).
//!
//! Services secondaires (content_discovery, torrent_checker, rss,
//! watch_folder) : ajoutes a l'etape 14 du roadmap.

pub mod config;
pub mod error;
pub mod notifier;
pub mod session;

pub use config::CoreConfig;
pub use error::{CoreError, Result};
pub use notifier::{Notification, Notifier};
pub use session::CoreSession;

#[cfg(test)]
mod tests {
    use super::*;

    /// .torrent minimal produit par le bencode de tribler-format.
    fn test_torrent_bytes() -> Vec<u8> {
        let mut info = std::collections::BTreeMap::new();
        info.insert(b"length".to_vec(), tribler_format::bencode::BValue::Int(42));
        info.insert(
            b"name".to_vec(),
            tribler_format::bencode::BValue::Bytes(b"core-test.bin".to_vec()),
        );
        info.insert(
            b"piece length".to_vec(),
            tribler_format::bencode::BValue::Int(16384),
        );
        info.insert(
            b"pieces".to_vec(),
            tribler_format::bencode::BValue::Bytes(vec![0u8; 20]),
        );
        let mut root = std::collections::BTreeMap::new();
        root.insert(
            b"info".to_vec(),
            tribler_format::bencode::BValue::Dict(info),
        );
        tribler_format::bencode::encode(&tribler_format::bencode::BValue::Dict(root))
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
        // (la boucle tourne en tache de fond ; on attend un evenement).
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("aucune notification recue")
            .unwrap();
        assert!(matches!(n, Notification::DownloadProgress(_)));

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
