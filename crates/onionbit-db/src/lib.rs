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
pub mod conversations;
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

    /// Un `torrent_state` decouvert via gossip/checker sans
    /// `channel_node` associe rendait des colonnes `cn.*` NULL sous
    /// `LEFT JOIN` — `from_row` levait `FromSqlConversionFailure` et
    /// tout le lot populaire (et `count_entries`) renvoyait 500.
    #[test]
    fn popular_entries_ignore_essaim_sans_metadonnee() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            let frais = i64::MAX / 2;
            // Orphelin : sante seule, aucun channel_node — le plus
            // populaire de tous pour etre certain qu'il remonte.
            let orphelin = vec![0xcdu8; 20];
            health::upsert_torrent_state(c, &orphelin)?;
            health::update_torrent_health(c, &orphelin, 999, 9, frais, false)?;
            c.execute(
                "UPDATE torrent_state SET has_data = 1 WHERE infohash = ?1",
                rusqlite::params![orphelin],
            )?;
            // Torrent documente, moins populaire.
            let row = ChannelNodeRow {
                infohash: vec![2u8; 20],
                title: "documente".into(),
                metadata_type: 300,
                public_key: vec![8u8; 64],
                id_: 1,
                timestamp: 1_700_000_000_000,
                ..Default::default()
            };
            channel::insert(c, &row)?;
            health::update_torrent_health(c, &row.infohash, 5, 1, frais, false)?;
            c.execute(
                "UPDATE torrent_state SET has_data = 1 WHERE infohash = ?1",
                rusqlite::params![row.infohash],
            )?;

            let p = channel::SelectParams {
                popular: true,
                ..Default::default()
            };
            let entries = channel::select_entries(c, &p)?;
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].title, "documente");
            assert_eq!(channel::count_entries(c, &p)?, 1);
            Ok(())
        })
        .unwrap();
    }

    /// La branche `popular` court-circuitait `select_filtered` :
    /// `hide_xxx`/`category`/`tags` et `first..last` etaient ignores
    /// (100 lignes brutes, xxx compris).
    #[test]
    fn popular_entries_respecte_hide_xxx_et_pagination() {
        let db = Database::memory().unwrap();
        db.with(|c| {
            let frais = i64::MAX / 2;
            // 3 torrents documentes dont 1 marque xxx.
            for i in 1u8..=3 {
                let row = ChannelNodeRow {
                    infohash: vec![i; 20],
                    title: format!("t{i}"),
                    metadata_type: 300,
                    public_key: vec![i; 64],
                    id_: i64::from(i),
                    timestamp: 1_700_000_000_000,
                    xxx: if i == 3 { 1.0 } else { 0.0 },
                    ..Default::default()
                };
                channel::insert(c, &row)?;
                health::update_torrent_health(c, &row.infohash, i64::from(i), 0, frais, false)?;
                c.execute(
                    "UPDATE torrent_state SET has_data = 1 WHERE infohash = ?1",
                    rusqlite::params![row.infohash],
                )?;
            }
            let sans_xxx = channel::SelectParams {
                popular: true,
                hide_xxx: true,
                ..Default::default()
            };
            let entries = channel::select_entries(c, &sans_xxx)?;
            assert_eq!(entries.len(), 2);
            assert_eq!(channel::count_entries(c, &sans_xxx)?, 2);
            // Pagination `pony_query[first-1:last]` : first=2,last=2
            // -> exactement la 2e ligne ; le compte reste non pagine.
            let page = channel::SelectParams {
                popular: true,
                first: 2,
                last: Some(2),
                ..Default::default()
            };
            assert_eq!(channel::select_entries(c, &page)?.len(), 1);
            assert_eq!(channel::count_entries(c, &page)?, 3);
            // Filtre tag : meme predicat que `select_filtered`
            // (`instr(tags, 'video,') > 0` — le LIKE %suffixe seul
            // ne matchait pas 'video,').
            c.execute(
                "UPDATE channel_node SET tags = 'video,' WHERE title = 't1'",
                [],
            )?;
            let tagge = channel::SelectParams {
                popular: true,
                tags: vec!["video".into()],
                ..Default::default()
            };
            let hits = channel::select_entries(c, &tagge)?;
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].title, "t1");
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
