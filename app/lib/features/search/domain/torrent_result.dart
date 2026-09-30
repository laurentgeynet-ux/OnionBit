/// Origine d'un résultat de recherche.
enum TorrentSource {
  /// Base de métadonnées locale (`/api/metadata/search/local`,
  /// `popular`).
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

  /// Magnet minimal pour l'ajout direct.
  String get magnet =>
      'magnet:?xt=urn:btih:$infohash&dn=${Uri.encodeComponent(name)}';

  /// Copie marquée « réseau » — pour les entrées qui apparaissent en
  /// base locale suite à une recherche distante (le backend intègre
  /// les `SelectResponse` dans `channel_node` sans événement dédié).
  TorrentResult asRemote() => TorrentResult(
    infohash: infohash,
    name: name,
    size: size,
    source: TorrentSource.remote,
    seeders: seeders,
    leechers: leechers,
    date: date,
  );
}
