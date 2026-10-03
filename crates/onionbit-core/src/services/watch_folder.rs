// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `watch_folder` — equivalent de `tribler/core/watch_folder/
//! manager.py` : surveille un repertoire et importe les `.torrent`
//! / `.magnet` nouveaux comme telechargements.
//!
//! Dedup par info-hash (le moteur connais deja le torrent => ignore)
//! et par chemin deja traite dans le cycle courant.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::session::CoreSession;

/// Extensions importees (`.magnet` contient l'URI en clair).
const WATCHED_EXTENSIONS: [&str; 2] = ["torrent", "magnet"];

/// Intervalle par defaut de scan (`watch_folder/check_interval`
/// Python).
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(10);

/// Surveillant de repertoire : un `tick` importe les fichiers neufs.
#[derive(Clone)]
pub struct WatchFolderService {
    session: CoreSession,
    /// Repertoire surveille (recursif, comme `os.walk` Python).
    dir: PathBuf,
    /// Intervalle entre deux scans.
    interval: Duration,
    /// Arret propre.
    stop: Arc<tokio::sync::watch::Sender<bool>>,
}

impl std::fmt::Debug for WatchFolderService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchFolderService")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl WatchFolderService {
    /// Cree le service (sans le demarrer).
    pub fn new(session: CoreSession, dir: PathBuf, interval: Duration) -> Self {
        let (stop, _) = tokio::sync::watch::channel(false);
        Self {
            session,
            dir,
            interval,
            stop: Arc::new(stop),
        }
    }

    /// Lance le scan periodique (`register_task` Python).
    /// Repertoire surveille (pour `apply_service_settings` —
    /// redemarrage si le chemin change).
    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// Intervalle de scan (pour `apply_service_settings` —
    /// redemarrage si la cadence change).
    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn start(&self) {
        let svc = self.clone();
        let mut stop_rx = self.stop.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(svc.interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = tick.tick() => { svc.check().await; }
                }
            }
        });
    }

    /// Arrete le scan periodique.
    pub fn stop(&self) {
        // `send_replace` : `send` echouerait si la tache n'a pas encore
        // souscrit — le signal d'arret serait perdu (cf. session.rs,
        // canal `restore_done`).
        self.stop.send_replace(true);
    }

    /// Un cycle de scan : importe les fichiers eligibles nouveaux.
    /// Retourne le nombre de fichiers traites (diagnostic/tests).
    pub async fn check(&self) -> usize {
        if !self.dir.is_dir() {
            tracing::warn!(dir = %self.dir.display(), "watch_folder: repertoire absent");
            return 0;
        }
        let mut processed = 0usize;
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack = vec![self.dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !seen.insert(path.clone()) {
                    continue;
                }
                let is_watched = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| WATCHED_EXTENSIONS.contains(&e));
                if !is_watched {
                    continue;
                }
                processed += 1;
                self.process_file(&path).await;
            }
        }
        processed
    }

    /// `process_torrent_file` : `.torrent` -> octets, `.magnet` -> URI.
    async fn process_file(&self, path: &Path) {
        match path.extension().and_then(|e| e.to_str()) {
            Some("torrent") => {
                let Ok(bytes) = std::fs::read(path) else {
                    tracing::warn!(path = %path.display(), "watch_folder: lecture impossible");
                    return;
                };
                // Dedup par info-hash : parse borne en amont.
                if let Ok(meta) = onionbit_format::torrent::TorrentMeta::parse(&bytes) {
                    let ih = onionbit_crypto::hash::to_hex(&meta.info_hash);
                    if self.session.engine().get(&ih).is_some() {
                        return;
                    }
                }
                if let Err(e) = self.session.add_torrent_bytes(bytes, false).await {
                    tracing::warn!(path = %path.display(), error = %e, "watch_folder: ajout echoue");
                }
            }
            Some("magnet") => {
                let Ok(uri) = std::fs::read_to_string(path) else {
                    return;
                };
                let uri = uri.trim();
                if uri.is_empty() {
                    return;
                }
                if let Err(e) = self.session.add_download(uri, false).await {
                    tracing::warn!(path = %path.display(), error = %e, "watch_folder: ajout magnet echoue");
                }
            }
            _ => {}
        }
    }
}

/// Supprime le fichier source (`*.torrent` / `*.magnet`) d'un
/// telechargement dont l'info-hash est `ih_hex` dans le repertoire
/// surveille — sinon le scan suivant le re-importerait
/// automatiquement. Bloquant (fs sync) : appeler via
/// `tokio::task::spawn_blocking`. Ne touche que `dir`.
pub fn remove_source(dir: &Path, ih_hex: &str) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let matches = match path.extension().and_then(|e| e.to_str()) {
                Some("torrent") => std::fs::read(&path)
                    .ok()
                    .and_then(|b| onionbit_format::torrent::TorrentMeta::parse(&b).ok())
                    .is_some_and(|m| {
                        onionbit_crypto::hash::to_hex(&m.info_hash).eq_ignore_ascii_case(ih_hex)
                    }),
                Some("magnet") => std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| onionbit_format::magnet::MagnetLink::parse(s.trim()).ok())
                    .is_some_and(|m| m.info_hash_hex().eq_ignore_ascii_case(ih_hex)),
                _ => false,
            };
            if matches {
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "watch_folder: source supprimee");
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "watch_folder: source non supprimable");
                    }
                }
            }
        }
    }
}
