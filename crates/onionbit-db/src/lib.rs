// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-db` — persistance SQLite du daemon.
//!
//! Equivalent de `tribler.core.database` (Pony ORM -> SQLite) : sante
//! des essaims (`torrent_state`), trackers (`tracker_state`),
//! metadonnees de canaux (`channel_node`), telechargements connus
//! (`downloads`), cle/valeur (`misc`).
//!
//! Le schema reprend la semantique des entites Pony v15 cote Python,
//! avec `datetime` → `INTEGER` (secondes Unix) et `bool` → 0/1. Les
//! migrations sont versionnees via `PRAGMA user_version`
//! (cf. `migrations.rs`) ; la version applicative Python
//! (`MiscData.db_version`, actuellement 15) est portee dans `misc`
//! pour information mais la compatibilite binaire avec les bases
//! Python n'est **pas** un objectif.

pub mod attestations;
pub mod channel;
pub mod db;
pub mod downloads;
pub mod error;
pub mod ext_ledger;
pub mod guards;
pub mod health;
pub mod messaging;
pub mod migrations;
pub mod misc;
pub mod models;
pub mod peer_stats;
pub mod peers;
pub mod pex;
pub mod ranks;
pub mod rss;

pub use db::Database;
pub use error::{DbError, Result};
pub use guards::GuardRow;
pub use messaging::{MsgContactRow, MsgMessageRow};
pub use models::{ChannelNodeRow, DownloadRow, TorrentStateRow, TrackerStateRow};
pub use peer_stats::PeerStatRow;
pub use peers::Ipv8PeerRow;
pub use pex::PexRow;
pub use rss::RssItemRow;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_creent_le_schema_v1() {
        let db = Database::memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), migrations::SCHEMA_VERSION);
        // Les tables existent.
        db.with(|c| {
            let n: i64 = c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            assert!(n >= 5, "tables manquantes: {n}");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn misc_cle_valeur() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            assert_eq!(misc::get(c, "db_version")?, None);
            misc::set(c, "db_version", "1")?;
            assert_eq!(misc::get(c, "db_version")?, Some("1".into()));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn torrent_state_upsert_et_health() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            let ih = vec![0xabu8; 20];
            let ts = health::upsert_torrent_state(c, &ih)?;
            assert_eq!(ts.seeders, 0);
            health::update_torrent_health(c, &ih, 12, 3, 1_700_000_000, true)?;
            let ts = health::get_torrent_state(c, &ih)?.unwrap();
            assert_eq!((ts.seeders, ts.leechers), (12, 3));
            assert!(ts.self_checked);
            health::link_tracker(c, &ih, "udp://tracker.local:80")?;
            assert_eq!(health::trackers_of(c, &ih)?, vec!["udp://tracker.local:80"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn channel_node_insert_et_dedup() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            let row = ChannelNodeRow {
                infohash: vec![1u8; 20],
                title: "titre".into(),
                metadata_type: 300,
                public_key: vec![9u8; 64],
                id_: 42,
                timestamp: 1_700_000_000_000,
                ..Default::default()
            };
            let r1 = channel::insert(c, &row)?.unwrap();
            assert!(r1 > 0);
            // Doublon (public_key, id_) : deduplique.
            assert_eq!(channel::insert(c, &row)?, None);
            let found = channel::get_by_pk_id(c, &row.public_key, 42)?.unwrap();
            assert_eq!(found.title, "titre");
            // L'infohash a cree un torrent_state associe.
            assert!(found.health_rowid.is_some());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn downloads_cycle() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            let ih = vec![7u8; 20];
            let row = DownloadRow {
                infohash: ih.clone(),
                name: Some("test".into()),
                source_uri: "magnet:?xt=urn:btih:x".into(),
                output_dir: "dl".into(),
                added_on: 1_700_000_000,
                ..Default::default()
            };
            downloads::upsert(c, &row)?;
            assert_eq!(downloads::list(c)?.len(), 1);
            downloads::set_finished(c, &ih, true)?;
            assert!(downloads::get(c, &ih)?.unwrap().finished);
            downloads::delete(c, &ih)?;
            assert!(downloads::get(c, &ih)?.is_none());
            Ok(())
        })
        .unwrap();
    }
}
