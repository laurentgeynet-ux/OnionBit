import 'torrent_result.dart';

/// Requête distante acceptée par le daemon (`PUT /api/search/remote`).
class RemoteQuery {
  const RemoteQuery({required this.requestUuid, required this.peers});

  /// UUID de la requête — filtre les événements SSE
  /// `remote_query_results`.
  final String requestUuid;

  /// Pairs auxquels la requête a été diffusée (mids hex).
  final List<String> peers;
}

/// Contrat du dépôt recherche (`/api/metadata/*`, `/api/search/remote`).
abstract interface class SearchRepository {
  /// Torrents les plus populaires connus (contenu initial de la page
  /// recherche quand la requête est vide).
  Future<List<TorrentResult>> popular({int limit = 50});

  /// Recherche FTS locale dans `metadata.db`.
  Future<List<TorrentResult>> searchLocal(String query);

  /// Diffuse la requête aux pairs — réponses poussées via SSE.
  Future<RemoteQuery> searchRemote(String query);
}
