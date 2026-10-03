// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Options par telechargement a l'ajout — traduction Rust du
//! `DownloadConfig` checkpointe par Tribler (`dlcheckpoints/`).
//!
//! Librqbit n'expose ces reglages qu'a l'ajout du torrent : la couche
//! domaine (`onionbit-core`) les persiste en base et les reapplique a
//! chaque (re)creation du telechargement (restauration, `update_hops`,
//! `recheck`, `move_storage`).

use std::path::PathBuf;

/// Reglages par telechargement appliques a l'ajout.
#[derive(Debug, Clone, Default)]
pub struct AddDownloadOptions {
    /// Demarrer en pause (`user_stopped` persiste).
    pub paused: bool,
    /// Dossier de sortie explicite (`None` = dossier de session).
    /// Parite libtorrent : pour un torrent multi-fichiers, le
    /// sous-dossier `<nom du torrent>` y est ajoute par rqbit
    /// (`name_subfolder`), sauf si `output_includes_name` est vrai.
    pub output_folder: Option<PathBuf>,
    /// `true` quand `output_folder` designe deja le dossier final
    /// (nom du torrent inclus) — typiquement au restore, ou le
    /// `output_dir` persiste provient de `Download::output_folder()`
    /// qui retourne le dossier complet. Evite le chemin imbrique
    /// `Nom\Nom` a la restauration.
    pub output_includes_name: bool,
    /// Indices des fichiers a telecharger (`None` = tous ;
    /// `Some(vec![])` = aucun — comme `selected_files` Python).
    pub only_files: Option<Vec<usize>>,
    /// Trackers additionnels (`extra_trackers` persistes — rqbit les
    /// fusionne a `announce`/`announce-list` a l'ajout).
    pub trackers: Vec<String>,
    /// Limite d'upload par torrent en octets/s (`None` = illimite).
    /// Appliquee via `initial ratelimits` rqbit a l'ajout — pas de
    /// mutation a chaud exposee par librqbit 9.x (ecart documente).
    pub upload_limit_bps: Option<u64>,
    /// Limite de download par torrent en octets/s.
    pub download_limit_bps: Option<u64>,
    /// Pairs d'amorcage injectes a l'ajout (`initial_peers` rqbit) :
    /// le telechargement demarre avec ces adresses, avant tout
    /// resultat DHT/tracker — utilise par les bancs live (seeder
    /// loopback injecte) et le hidden seeding d'introduction.
    pub initial_peers: Vec<std::net::SocketAddr>,
}
