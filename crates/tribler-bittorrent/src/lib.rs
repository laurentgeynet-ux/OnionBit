//! `tribler-bittorrent` — moteur BitTorrent.
//!
//! Responsabilite unique : exposer une API Tribler-idiomatique
//! ([`BtEngine`], [`Download`], [`DownloadStats`], [`DownloadState`])
//! au-dessus du moteur BitTorrent reutilise `librqbit` (ADR-0001) :
//! bencode, protocole peer-wire, DHT mainline (BEP 5), uTP, trackers
//! HTTP/UDP.
//!
//! Ce crate est l'equivalent du module Python
//! `tribler.core.libtorrent`, mais ne reimplemente pas le protocole
//! filaire bas niveau : il pilote `librqbit::Session` et traduit son
//! etat vers les types du domaine.
//!
//! Point d'integration futur avec `tribler-tunnel` : `EngineConfig::
//! socks5_proxy` force le trafic pair sortant a travers le proxy SOCKS5
//! local expose par un circuit anonyme (cf. `tribler-network-policy`
//! pour les garde-fous — loopback uniquement).

pub mod config;
pub mod download;
pub mod engine;
pub mod error;

pub use config::EngineConfig;
pub use download::{Download, DownloadState, DownloadStats};
pub use engine::BtEngine;
pub use error::{BtError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_offline_demarre_et_ajoute_un_torrent() {
        let dir = tempfile::tempdir().unwrap();
        let engine = BtEngine::start(EngineConfig::offline(dir.path().join("dl")))
            .await
            .unwrap();

        // .torrent minimal construit avec le bencode de tribler-format.
        let mut info = std::collections::BTreeMap::new();
        info.insert(b"length".to_vec(), tribler_format::bencode::BValue::Int(42));
        info.insert(
            b"name".to_vec(),
            tribler_format::bencode::BValue::Bytes(b"test.bin".to_vec()),
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
        let bytes = tribler_format::bencode::encode(&tribler_format::bencode::BValue::Dict(root));

        let dl = engine.add_torrent_bytes(bytes, true).await.unwrap();
        assert_eq!(dl.name().as_deref(), Some("test.bin"));
        let stats = dl.stats();
        assert_eq!(stats.total_bytes, 42);
        assert_eq!(engine.list().len(), 1);

        engine.stop().await;
    }
}
