// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Aperçu d'un `.torrent` avant ajout (`PUT /api/torrentinfo/file`).
///
/// Les champs `trackers` et `isPrivate` sont une extension par rapport
/// à la réponse Python : ils permettent d'anticiper les annonces
/// impossibles via les tunnels anonymes.
class TorrentPreview {
  const TorrentPreview({
    required this.name,
    required this.trackers,
    required this.isPrivate,
  });

  final String name;
  final List<String> trackers;
  final bool isPrivate;
}
