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

  /// Backend externe `.onion` via Tor (`GET /api/search/external`,
  /// ADR-0028 §5) — résultats scrapés (TPB & co), `sourceTag`
  /// précise le backend (`tpb`).
  onion,
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
    this.sourceTag,
    this.magnetUri,
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

  /// Identifiant du backend d'origine pour les résultats `onion`
  /// (`"tpb"` — champ `source` du JSON externe ; `null` sinon).
  /// Affiché en badge Source à la place du libellé générique.
  final String? sourceTag;

  /// Magnet tel que publié par la source externe (`onion`) — prioritaire
  /// sur le magnet synthétisé : elle peut y inclure trackers et
  /// paramètres propres au site.
  final String? magnetUri;

  /// Magnet complet pour l'ajout direct avec trackers de la source.
  String get magnet {
    final uri = magnetUri;
    if (uri != null && uri.startsWith('magnet:')) return uri;
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
    sourceTag: sourceTag,
    magnetUri: magnetUri,
  );
}
