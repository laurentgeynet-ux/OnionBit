// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../../../core/config/ui_log.dart';
import '../domain/download.dart';
import '../domain/download_file.dart';
import '../domain/download_tracker.dart';
import '../domain/downloads_repository.dart';
import '../domain/torrent_preview.dart';
import 'download_dto.dart';

/// Implémentation REST du dépôt downloads (endpoints Python).
class RestDownloadsRepository implements DownloadsRepository {
  RestDownloadsRepository(this._api);

  final ApiClient _api;

  @override
  Future<List<Download>> list() async {
    // `get_peers=1` : la GUI Python poll ce flag pour la liste de
    // pairs de chaque download (onglet Pairs du panneau détail).
    final resp =
        await _api.get('/downloads', query: {'get_peers': '1'})
            as Map<String, dynamic>;
    final items = resp['downloads'] as List<dynamic>? ?? const [];
    return items
        .whereType<Map<String, dynamic>>()
        .map((d) => d.toDownload())
        .toList();
  }

  @override
  Future<String> add({
    String? uri,
    String? torrentPath,
    String? destination,
    String? area,
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  }) async {
    final kind = uri != null && uri.startsWith('magnet:')
        ? 'magnet'
        : uri != null
            ? 'uri'
            : 'torrent';
    try {
      final resp = await _api.put(
        '/downloads',
        body: {
          'uri': ?uri,
          'torrent': ?torrentPath,
          'destination': ?destination,
          'area': ?area,
          if (anonHops > 0) 'anon_hops': anonHops,
          'safe_seeding': safeSeeding,
          'paused': paused,
        },
      ) as Map<String, dynamic>;
      final infohash = (resp['infohash'] as String?) ?? '';
      uiLog('ajout $kind accepte infohash=$infohash hops=$anonHops');
      return infohash;
    } catch (e) {
      uiLog('ajout $kind en echec : $e');
      rethrow;
    }
  }

  @override
  Future<String> addTorrentBytes(
    List<int> bytes, {
    String? destination,
    String? area,
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  }) async {
    try {
      final resp = await _api.putTorrent(
        '/downloads',
        bytes,
        query: {
          if (destination != null && destination.isNotEmpty)
            'destination': destination,
          'area': ?area,
          if (anonHops > 0) 'anon_hops': '$anonHops',
          'safe_seeding': '$safeSeeding',
          'paused': '$paused',
        },
      ) as Map<String, dynamic>;
      final infohash = (resp['infohash'] as String?) ?? '';
      uiLog('ajout .torrent accepte infohash=$infohash hops=$anonHops');
      return infohash;
    } catch (e) {
      uiLog('ajout .torrent en echec : $e');
      rethrow;
    }
  }

  @override
  Future<TorrentPreview> previewTorrentFile(List<int> bytes) async {
    final resp = await _api.putTorrent('/torrentinfo/file', bytes)
        as Map<String, dynamic>;
    final trackers = (resp['trackers'] as List<dynamic>? ?? const [])
        .whereType<String>()
        .toList();
    return TorrentPreview(
      name: (resp['name'] as String?) ?? '',
      trackers: trackers,
      isPrivate: resp['private'] == true,
    );
  }

  @override
  Future<List<DownloadFile>> files(String infohash) async {
    final resp =
        await _api.get('/downloads/$infohash/files') as Map<String, dynamic>;
    final items = resp['files'] as List<dynamic>? ?? const [];
    return [
      for (final f in items.whereType<Map<String, dynamic>>())
        DownloadFile(
          index: (f['index'] as num?)?.toInt() ?? 0,
          name: (f['name'] as String?) ?? '',
          size: (f['size'] as num?)?.toInt() ?? 0,
          included: f['included'] != false,
          // `progress` est une fraction 0..1 (comme Python), pas des
          // octets.
          progress: (f['progress'] as num?)?.toDouble() ?? 0,
        ),
    ];
  }

  @override
  Future<List<DownloadTracker>> trackers(String infohash) async {
    final resp =
        await _api.get('/downloads/$infohash/trackers') as Map<String, dynamic>;
    final items = resp['tracker_info'] as List<dynamic>? ?? const [];
    return [
      for (final t in items.whereType<Map<String, dynamic>>())
        DownloadTracker(
          url: (t['url'] as String?) ?? '',
          status: (t['status'] as String?) ?? 'Not contacted yet',
          peers: (t['peers'] as num?)?.toInt() ?? -1,
          seeds: (t['seeds'] as num?)?.toInt() ?? -1,
          leeches: (t['leeches'] as num?)?.toInt() ?? -1,
        ),
    ];
  }

  @override
  Future<void> addTracker(String infohash, String url) =>
      _api.put('/downloads/$infohash/trackers', body: {'url': url});

  @override
  Future<void> removeTracker(String infohash, String url) =>
      _api.delete('/downloads/$infohash/trackers', body: {'url': url});

  @override
  Future<void> addDefaultTrackers(String infohash) =>
      _api.put('/downloads/$infohash/default_trackers');

  @override
  Future<void> forceTrackerAnnounce(String infohash, String url) =>
      _api.put('/downloads/$infohash/tracker_force_announce', body: {'url': url});

  @override
  Future<void> pause(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'state': 'stop'});

  @override
  Future<void> resume(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'state': 'resume'});

  @override
  Future<void> setAnonHops(String infohash, int hops) =>
      _api.patch('/downloads/$infohash', body: {'anon_hops': hops});

  @override
  Future<void> moveInQueue(String infohash, QueueOp op) =>
      _api.patch('/downloads/$infohash', body: {'queue_position': op.wire});

  @override
  Future<void> setAutoManaged(String infohash, bool enabled) =>
      _api.patch('/downloads/$infohash', body: {'auto_managed': enabled});

  @override
  Future<void> setRateLimits(
    String infohash, {
    int? uploadLimit,
    int? downloadLimit,
  }) =>
      _api.patch('/downloads/$infohash', body: {
        'upload_limit': ?uploadLimit,
        'download_limit': ?downloadLimit,
      });

  @override
  Future<void> setSeedingRatio(String infohash, double ratio) =>
      _api.patch('/downloads/$infohash', body: {'seeding_ratio': ratio});

  @override
  Future<void> resetSeedingRatio(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'seeding_ratio_default': true});

  @override
  Future<String> clonePublic(String infohash, {int? anonHops}) async {
    try {
      final resp = await _api.post(
        '/downloads/$infohash/clone_public',
        body: {'anon_hops': ?anonHops},
      ) as Map<String, dynamic>;
      final ih = (resp['infohash'] as String?) ?? '';
      uiLog('jumeau public cree infohash=$ih (depuis $infohash)');
      return ih;
    } catch (e) {
      uiLog('clone public en echec : $e');
      rethrow;
    }
  }

  @override
  Future<void> recheck(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'state': 'recheck'});

  @override
  Future<void> moveStorage(
    String infohash, {
    required String destination,
    String? completedDir,
  }) =>
      _api.patch('/downloads/$infohash', body: {
        'state': 'move_storage',
        'dest_dir': destination,
        'completed_dir': ?completedDir,
      });

  @override
  Future<void> setSelectedFiles(String infohash, List<int> indices) =>
      _api.patch('/downloads/$infohash', body: {'selected_files': indices});

  @override
  Future<void> setFilePriority(String infohash, int index, int priority) =>
      _api.patch('/downloads/$infohash', body: {
        'file_priority': [index, priority],
      });

  @override
  Future<void> remove(String infohash, {bool deleteFiles = false}) =>
      _api.delete('/downloads/$infohash', body: {'remove_data': deleteFiles});

  @override
  Future<({String infohash, String path})> createTorrent({
    required List<String> files,
    String? name,
    String? description,
    String? tracker,
    String? exportDir,
  }) async {
    try {
      final resp = await _api.post(
        '/createtorrent',
        body: {
          'files': files,
          'name': ?name,
          'description': ?description,
          'tracker': ?tracker,
          'export_dir': ?exportDir,
        },
      ) as Map<String, dynamic>;
      final results = resp['results'] as List<dynamic>? ?? const [];
      final first =
          results.isNotEmpty && results.first is Map<String, dynamic>
              ? results.first as Map<String, dynamic>
              : const <String, dynamic>{};
      final infohash = (first['infohash'] as String?) ?? '';
      final path = (first['path'] as String?) ?? '';
      uiLog('creation torrent infohash=$infohash path=$path');
      return (infohash: infohash, path: path);
    } catch (e) {
      uiLog('creation torrent en echec : $e');
      rethrow;
    }
  }
}
