// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../../../core/config/ui_log.dart';
import '../domain/search_repository.dart';
import '../domain/torrent_result.dart';

/// Implémentation REST du dépôt recherche.
class RestSearchRepository implements SearchRepository {
  RestSearchRepository(this._api);

  final ApiClient _api;

  static List<Map<String, dynamic>> _results(dynamic resp) =>
      ((resp as Map<String, dynamic>)['results'] as List?)
          ?.whereType<Map<String, dynamic>>()
          .toList() ??
      const [];

  /// `created`/`torrent_date` : epoch **secondes** partout ;
  /// `updated` : epoch **millisecondes** chez les pairs Tribler
  /// Python (timestamp signé `>Q` des métadonnées), secondes chez
  /// OnionBit — heuristique de magnitude pour les deux mondes.
  static DateTime? _parseDate(dynamic v) {
    if (v is num) {
      var ms = v.toInt();
      // < 1e11 = secondes (1e11 s ≈ année 5138 ; une valeur ms
      // réaliste est >= 9.8e11, l'ère BitTorrent débutant en 2001).
      if (ms.abs() < 100000000000) ms *= 1000;
      return DateTime.fromMillisecondsSinceEpoch(ms);
    }
    if (v is String) return DateTime.tryParse(v);
    return null;
  }

  static List<String> _parseTrackers(
    dynamic rawTrackers,
    dynamic rawTrackerInfo,
  ) {
    final list = <String>[];
    if (rawTrackers is List) {
      for (final t in rawTrackers) {
        if (t is String && t.trim().isNotEmpty) {
          final trimmed = t.trim();
          if (!list.contains(trimmed)) list.add(trimmed);
        }
      }
    }
    if (rawTrackerInfo is String && rawTrackerInfo.trim().isNotEmpty) {
      final trimmed = rawTrackerInfo.trim();
      if (!list.contains(trimmed)) list.add(trimmed);
    }
    return list;
  }

  static TorrentResult _parse(Map<String, dynamic> j, TorrentSource source) =>
      TorrentResult(
        infohash: '${j['infohash'] ?? ''}',
        name: '${j['name'] ?? ''}',
        size: ((j['size'] ?? j['length']) as num?)?.toInt() ?? 0,
        source: source,
        seeders: (j['num_seeders'] as num?)?.toInt(),
        leechers: (j['num_leechers'] as num?)?.toInt(),
        // `created` (date de création du torrent, secondes) d'abord —
        // plus parlante que `updated` (mise à jour du noeud chez le
        // pair émetteur, unité ms chez Python).
        date: _parseDate(j['created'] ?? j['updated'] ?? j['torrent_date']),
        trackers: _parseTrackers(j['trackers'], j['tracker_info']),
      );

  /// Parse une entrée `remote_query_results` (même forme `results`).
  static List<TorrentResult> parseRemoteResults(Map<String, dynamic> event) => [
    for (final j in _results(event)) _parse(j, TorrentSource.remote),
  ];

  @override
  Future<List<TorrentResult>> searchLocal(
    String query, {
    String? sortBy,
    bool sortDesc = true,
  }) async => [
    for (final j in _results(
      await _api.get(
        '/metadata/search/local',
        query: {
          'fts_text': query,
          'sort_by': ?sortBy,
          'sort_desc': '$sortDesc',
        },
      ),
    ))
      _parse(j, TorrentSource.local),
  ];

  @override
  Future<RemoteQuery> searchRemote(String query) async {
    try {
      final resp = await _api.put(
        '/search/remote',
        query: {
          'fts_text': query,
          // `max_response_size` = 100 côté Python comme côté
          // OnionBit : au-delà le pair émetteur tronque la plage
          // `first..last` — 100 est le plafond utile du protocole.
          'last': '100',
        },
      ) as Map<String, dynamic>;
      uiLog(
        'recherche distante "$query" -> uuid=${resp['request_uuid']} '
        'peers=${(resp['peers'] as List?)?.length ?? 0}',
      );
      return RemoteQuery(
        requestUuid: '${resp['request_uuid'] ?? ''}',
        peers: [for (final p in (resp['peers'] as List?) ?? const []) '$p'],
      );
    } catch (e) {
      uiLog('recherche distante "$query" en echec : $e');
      rethrow;
    }
  }

  @override
  Future<({int seeders, int leechers})?> health(String infohash) async {
    final resp = await _api.get(
      '/metadata/torrents/$infohash/health',
      query: {'refresh': '1'},
    ) as Map<String, dynamic>;
    final h = resp['health'];
    if (h is! Map<String, dynamic>) return null; // « checking »
    return (
      seeders: (h['seeders'] as num?)?.toInt() ?? 0,
      leechers: (h['leechers'] as num?)?.toInt() ?? 0,
    );
  }

}
