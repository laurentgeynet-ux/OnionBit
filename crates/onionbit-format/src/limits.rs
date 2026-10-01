// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bornes de parsing (protection contre les entrees malveillantes).
//!
//! Source de verite unique pour les limites de `onionbit-format`
//! (cf. AGENTS.md, "Aucune valeur en dur").

/// Profondeur maximale d'imbrication bencode (listes/dicts imbriques).
/// Une entree recursive profonde peut epuiser la pile.
pub const MAX_BENCODE_DEPTH: usize = 32;

/// Taille maximale d'un fichier `.torrent` (8 Mo — largement suffisant,
/// un torrent standard fait quelques Ko a quelques centaines de Ko).
pub const MAX_TORRENT_FILE_SIZE: usize = 8 * 1024 * 1024;

/// Longueur maximale d'un lien magnet (protection URI abusive).
pub const MAX_MAGNET_LEN: usize = 32 * 1024;

/// Taille maximale d'une valeur bencode encodee en sortie (protection
/// contre les re-serialisations explosives).
pub const MAX_BENCODE_VALUE_SIZE: usize = 16 * 1024 * 1024;
