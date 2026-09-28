import '../../../core/api/api_client.dart';
import '../domain/download.dart';
import '../domain/download_file.dart';
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
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  }) async {
    final resp = await _api.putTorrent(
      '/downloads',
      bytes,
      query: {
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
          priority: (f['priority'] as num?)?.toInt() ?? 0,
          progress: (f['progress'] as num?)?.toInt() ?? 0,
        ),
    ];
  }

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
