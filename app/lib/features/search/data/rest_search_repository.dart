import '../../../core/api/api_client.dart';
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

  static TorrentResult _parse(Map<String, dynamic> j, TorrentSource source) =>
      TorrentResult(
        infohash: '${j['infohash'] ?? ''}',
        name: '${j['name'] ?? ''}',
        size: ((j['size'] ?? j['length']) as num?)?.toInt() ?? 0,
        source: source,
        seeders: (j['num_seeders'] as num?)?.toInt(),
        leechers: (j['num_leechers'] as num?)?.toInt(),
      );

  /// Parse une entrée `remote_query_results` (même forme `results`).
  static List<TorrentResult> parseRemoteResults(Map<String, dynamic> event) => [
    for (final j in _results(event)) _parse(j, TorrentSource.remote),
  ];

  @override
  Future<List<TorrentResult>> popular({int limit = 50}) async => [
    for (final j in _results(
      await _api.get(
        '/metadata/torrents/popular',
        query: {'first': '1', 'last': '$limit'},
      ),
    ))
      _parse(j, TorrentSource.local),
  ];

  @override
  Future<List<TorrentResult>> searchLocal(String query) async => [
    for (final j in _results(
      await _api.get('/metadata/search/local', query: {'fts_text': query}),
    ))
      _parse(j, TorrentSource.local),
  ];

  @override
  Future<RemoteQuery> searchRemote(String query) async {
    final resp = await _api.put(
      '/search/remote',
      query: {'fts_text': query},
    ) as Map<String, dynamic>;
    return RemoteQuery(
      requestUuid: '${resp['request_uuid'] ?? ''}',
      peers: [for (final p in (resp['peers'] as List?) ?? const []) '$p'],
    );
  }
}
