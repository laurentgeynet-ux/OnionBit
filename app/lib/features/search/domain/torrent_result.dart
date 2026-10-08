// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Origine d'un résultat de recherche.
enum TorrentSource {
  /// Base de métadonnées locale (`/api/metadata/search/local`).
  local,

  /// Réponses des pairs de `ContentDiscoveryCommunity`, poussées via
  /// SSE (`remote_query_results`).
  remote,
}

/// Torrent découvert — forme commune des résultats `metadata` et des
/// réponses distantes.
class TorrentResult {
  const TorrentResult({
    required this.infohash,
    required this.name,
    required this.size,
    required this.source,
    this.seeders,
    this.leechers,
    this.date,
    this.trackers = const [],
  });

  final String infohash;
  final String name;
  final int size;
  final TorrentSource source;
  final int? seeders;
  final int? leechers;

  /// Date du torrent (`updated`/`torrent_date` du backend, epoch
  /// secondes ; `null` = inconnu).
  final DateTime? date;

  /// Trackers connus associés à ce torrent.
  final List<String> trackers;

  /// Magnet complet pour l'ajout direct avec trackers de la source.
  String get magnet {
    final base =
        'magnet:?xt=urn:btih:$infohash&dn=${Uri.encodeComponent(name)}';
    if (trackers.isEmpty) return base;
    final trParams =
        trackers.map((t) => '&tr=${Uri.encodeComponent(t)}').join();
    return '$base$trParams';
  }

  /// Copie avec la santé rafraîchie (sonde
  /// `/metadata/torrents/{ih}/health`).
  TorrentResult withHealth(int seeders, int leechers) => TorrentResult(
    infohash: infohash,
    name: name,
    size: size,
    source: source,
    seeders: seeders,
    leechers: leechers,
    date: date,
    trackers: trackers,
  );
}
