// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Acces a `rss_items` : entrees `.torrent` decouvertes par les
//! watchers RSS (`GET /api/rss` — extension Rust, cf. ADR-0006 ;
//! Tribler Python ne persiste pas les entrees).

use rusqlite::params;

use crate::Result;

/// Une entree RSS persiste (ligne de `rss_items`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssItemRow {
    /// URL du flux ayant produit l'entree.
    pub feed_url: String,
    /// URL `.torrent` de l'entree.
    pub link: String,
    /// Titre du torrent (`None` tant que la resolution n'a pas
    /// produit de metadonnees).
    pub title: Option<String>,
    /// Infohash hex (`None` idem).
    pub infohash: Option<String>,
    /// Premiere observation (secondes Unix).
    pub first_seen: i64,
}

impl crate::Database {
    /// Enregistre une entree nouvellement vue (idempotent :
    /// `INSERT OR IGNORE` sur la cle `(feed_url, link)`).
    pub fn insert_rss_item(&self, feed_url: &str, link: &str, first_seen: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO rss_items (feed_url, link, first_seen)
                 VALUES (?1, ?2, ?3)",
                params![feed_url, link, first_seen],
            )?;
            Ok(())
        })
    }

    /// Renseigne `title`/`infohash` apres resolution du `.torrent`.
    pub fn set_rss_item_metadata(
        &self,
        feed_url: &str,
        link: &str,
        title: &str,
        infohash: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE rss_items SET title = ?3, infohash = ?4
                 WHERE feed_url = ?1 AND link = ?2",
                params![feed_url, link, title, infohash],
            )?;
            Ok(())
        })
    }

    /// Liste les items, plus recents d'abord.
    pub fn list_rss_items(&self) -> Result<Vec<RssItemRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT feed_url, link, title, infohash, first_seen
                 FROM rss_items ORDER BY first_seen DESC, link",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(RssItemRow {
                    feed_url: r.get(0)?,
                    link: r.get(1)?,
                    title: r.get(2)?,
                    infohash: r.get(3)?,
                    first_seen: r.get(4)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rss_items_insert_meta_list() {
        let db = crate::Database::memory().unwrap();
        db.insert_rss_item("http://feed/a", "http://t/1.torrent", 100)
            .unwrap();
        // Idempotent sur la cle.
        db.insert_rss_item("http://feed/a", "http://t/1.torrent", 200)
            .unwrap();
        db.insert_rss_item("http://feed/b", "http://t/2.torrent", 150)
            .unwrap();

        db.set_rss_item_metadata("http://feed/a", "http://t/1.torrent", "T1", "ab")
            .unwrap();

        let items = db.list_rss_items().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].link, "http://t/2.torrent");
        assert_eq!(items[0].first_seen, 150);
        assert_eq!(items[1].title.as_deref(), Some("T1"));
        assert_eq!(items[1].infohash.as_deref(), Some("ab"));
        // `first_seen` conserve la premiere observation.
        assert_eq!(items[1].first_seen, 100);
    }
}
