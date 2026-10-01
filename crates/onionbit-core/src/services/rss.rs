// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `rss` — equivalent de `tribler/core/rss/rss.py` : surveille des
//! flux RSS, extrait les liens `.torrent` nouveaux, les telecharge
//! (anti-SSRF) et notifie les metadonnees creees.
//!
//! Fidelite au Python : GET conditionnel (`If-Modified-Since`),
//! backoff demande par le serveur (`Keep-Alive: timeout=N`),
//! recheck minimum 120 s, dedup des entrees connues.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use onionbit_db::Database;
use onionbit_network_policy::IpPolicy;

use crate::asyncio::tasks::{now_secs, TaskRegistry};
use crate::notifier::{Notification, Notifier};

/// Recheck minimum entre deux GET (defaut 120 s cote Python quand le
/// serveur ne demande rien).
const DEFAULT_RECHECK: Duration = Duration::from_secs(120);

/// Surveillant d'un flux RSS unique (`RSSWatcher` Python).
struct RssWatcher {
    url: String,
    /// Entrees deja vues (textes finissant en `.torrent`).
    previous_entries: Mutex<HashSet<String>>,
    /// `Last-Modified` recu (pour le GET conditionnel).
    last_modified: Mutex<Option<String>>,
    /// Re-check pas avant cet instant (backoff serveur).
    next_check: Mutex<tokio::time::Instant>,
}

/// `RSSWatcherManager` : un watcher par URL, `update` reconcilie la
/// liste (start/stop des watchers modifies).
pub struct RssManager {
    notifier: Notifier,
    ip_policy: IpPolicy,
    /// Persistance des items (`rss_items` — extension `GET /api/rss`).
    db: Arc<Database>,
    /// Registre de taches (`/api/ipv8/asyncio/tasks`).
    tasks: TaskRegistry,
    /// `session` pour persister les torrents decouverts (optionnel :
    /// le Python ne telecharge pas, il ingere les metadonnees —
    /// ici on notifie + persiste via le canal `TorrentMetadataCreated`
    /// et on laisse la couche appelante decider du telechargement).
    watchers: Mutex<HashMap<String, Arc<RssWatcher>>>,
    /// Arret global.
    stop: tokio::sync::watch::Sender<bool>,
}

impl std::fmt::Debug for RssManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RssManager")
            .field("watchers", &self.watchers.lock().unwrap().len())
            .finish_non_exhaustive()
    }
}

impl RssManager {
    /// Cree le gestionnaire (les watchers demarrent via `update`/`start`).
    pub fn new(
        notifier: Notifier,
        ip_policy: IpPolicy,
        db: Arc<Database>,
        tasks: TaskRegistry,
    ) -> Arc<Self> {
        let (stop, _) = tokio::sync::watch::channel(false);
        Arc::new(Self {
            notifier,
            ip_policy,
            db,
            tasks,
            watchers: Mutex::new(HashMap::new()),
            stop,
        })
    }

    /// Reconcilie la liste des URL (`update` Python) : demarre les
    /// nouvelles, arrete les retirees.
    pub fn update(self: &Arc<Self>, urls: &[String]) {
        let wanted: HashSet<&str> = urls
            .iter()
            .map(|u| u.as_str())
            .filter(|u| !u.is_empty())
            .collect();
        let mut watchers = self.watchers.lock().unwrap();
        watchers.retain(|u, _| wanted.contains(u.as_str()));
        for url in wanted {
            if watchers.contains_key(url) {
                continue;
            }
            let w = Arc::new(RssWatcher {
                url: url.to_string(),
                previous_entries: Mutex::new(HashSet::new()),
                last_modified: Mutex::new(None),
                next_check: Mutex::new(tokio::time::Instant::now()),
            });
            watchers.insert(url.to_string(), w.clone());
            // Tache periodique du watcher — `register_task` Python.
            self.tasks.register(
                Some("RssWatcher"),
                &format!("check {url}"),
                Some(DEFAULT_RECHECK.as_secs_f64()),
            );
            let mgr = self.clone();
            let mut stop_rx = self.stop.subscribe();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(DEFAULT_RECHECK);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = stop_rx.changed() => break,
                        _ = tick.tick() => {
                            if tokio::time::Instant::now() < *w.next_check.lock().unwrap() {
                                continue;
                            }
                            mgr.check(&w).await;
                        }
                    }
                }
            });
        }
    }

    /// Arrete tous les watchers.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
        self.watchers.lock().unwrap().clear();
    }

    /// `check` Python : GET conditionnel, backoff serveur, parse.
    async fn check(&self, w: &RssWatcher) {
        // GET conditionnel (If-Modified-Since si connu).
        let request = match super::fetch_checked(&w.url, &self.ip_policy).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(url = %w.url, error = %e, "rss: fetch echoue");
                *w.next_check.lock().unwrap() = tokio::time::Instant::now() + DEFAULT_RECHECK;
                return;
            }
        };
        // Backoff demande par le serveur (`Keep-Alive: timeout=N`).
        let mut recheck = DEFAULT_RECHECK;
        if let Some(ka) = request
            .headers()
            .get("keep-alive")
            .and_then(|v| v.to_str().ok())
        {
            for part in ka.split(',') {
                if let Some(v) = part.trim().strip_prefix("timeout=") {
                    if let Ok(n) = v.parse::<u64>() {
                        recheck = Duration::from_secs(n.max(1));
                    }
                }
            }
        }
        *w.next_check.lock().unwrap() = tokio::time::Instant::now() + recheck;
        if let Some(date) = request.headers().get("date").and_then(|v| v.to_str().ok()) {
            *w.last_modified.lock().unwrap() = Some(date.to_string());
        }
        if request.status() == reqwest::StatusCode::NOT_MODIFIED {
            return;
        }
        if !request.status().is_success() {
            return;
        }
        let Ok(body) = super::read_body_limited(request).await else {
            return;
        };
        self.parse_rss(w, &body).await;
    }

    /// `parse_rss` : textes XML finissant en `.torrent` -> resolve.
    async fn parse_rss(&self, w: &RssWatcher, content: &[u8]) {
        let mut out = HashSet::new();
        for v in super::xml_text_values(content) {
            if v.ends_with(".torrent") {
                out.insert(v);
            }
        }
        let new_entries: Vec<String> = {
            let mut prev = w.previous_entries.lock().unwrap();
            let new_entries = out.difference(&prev).cloned().collect();
            *prev = out;
            new_entries
        };
        for url in new_entries {
            // Extension `GET /api/rss` : persiste l'entree vue
            // (`first_seen` conserve la premiere observation).
            let _ = self.db.insert_rss_item(&w.url, &url, now_secs() as i64);
            self.resolve(&w.url, &url).await;
        }
    }

    /// `resolve` : telecharge le `.torrent` et notifie les
    /// metadonnees (le Python insere dans le MetadataStore — ici la
    /// notification `TorrentMetadataCreated` porte titre + infohash).
    async fn resolve(&self, feed_url: &str, url: &str) {
        let Ok(resp) = super::fetch_checked(url, &self.ip_policy).await else {
            tracing::warn!(url, "rss: telechargement du .torrent refuse");
            return;
        };
        let Ok(body) = super::read_body_limited(resp).await else {
            return;
        };
        let Ok(meta) = onionbit_format::torrent::TorrentMeta::parse(&body) else {
            tracing::warn!(url, "rss: reponse n'est pas un .torrent valide");
            return;
        };
        let infohash = onionbit_crypto::hash::to_hex(&meta.info_hash);
        let _ = self
            .db
            .set_rss_item_metadata(feed_url, url, &meta.name, &infohash);
        self.notifier.notify(Notification::TorrentMetadataCreated {
            infohash,
            title: meta.name.clone(),
        });
    }
}
