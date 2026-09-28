import '../../../core/api/api_client.dart';
import '../domain/download.dart';
import '../domain/download_file.dart';
import '../domain/download_tracker.dart';
import '../domain/downloads_repository.dart';
import 'download_dto.dart';

/// Implémentation REST du dépôt downloads (endpoints Python).
class RestDownloadsRepository implements DownloadsRepository {
  RestDownloadsRepository(this._api);

  final ApiClient _api;

  @override
  Future<List<Download>> list() async {
    final resp = await _api.get('/downloads') as Map<String, dynamic>;
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
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  }) async {
    final resp = await _api.put(
      '/downloads',
      body: {
        'uri': ?uri,
        'torrent': ?torrentPath,
        'destination': ?destination,
        if (anonHops > 0) 'anon_hops': anonHops,
        'safe_seeding': safeSeeding,
        'paused': paused,
      },
    ) as Map<String, dynamic>;
    return (resp['infohash'] as String?) ?? '';
  }

  @override
  Future<String> addTorrentBytes(
    List<int> bytes, {
    String? destination,
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  }) async {
    final resp = await _api.putTorrent(
      '/downloads',
      bytes,
      query: {
        if (destination != null && destination.isNotEmpty)
          'destination': destination,
        if (anonHops > 0) 'anon_hops': '$anonHops',
        'safe_seeding': '$safeSeeding',
        'paused': '$paused',
      },
    ) as Map<String, dynamic>;
    return (resp['infohash'] as String?) ?? '';
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
  Future<void> pause(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'state': 'stop'});

  @override
  Future<void> resume(String infohash) =>
      _api.patch('/downloads/$infohash', body: {'state': 'resume'});

  @override
  Future<void> setAnonHops(String infohash, int hops) =>
      _api.patch('/downloads/$infohash', body: {'anon_hops': hops});

  @override
  Future<void> remove(String infohash, {bool deleteFiles = false}) =>
      _api.delete('/downloads/$infohash', body: {'remove_data': deleteFiles});
}
