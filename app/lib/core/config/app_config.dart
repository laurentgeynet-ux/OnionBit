/// Configuration de connexion au daemon `tribler-daemon`.
///
/// V1 : daemon embarqué ou local, API sur `127.0.0.1:8085` (défaut du
/// daemon). `apiKey` correspond à `api.key` de la config Tribler —
/// vide par défaut (l'API Rust n'exige pas de clé sur loopback).
class AppConfig {
  const AppConfig({this.baseUrl = 'http://127.0.0.1:8085', this.apiKey = ''});

  /// URL de base de `tribler-api` (schéma + hôte + port, sans `/api`).
  final String baseUrl;

  /// Clé `X-Api-Key` si le daemon en exige une.
  final String apiKey;

  /// URI complète d'un chemin d'API (`/downloads`, …).
  Uri apiUri(String path, [Map<String, String>? query]) =>
      Uri.parse('$baseUrl/api$path').replace(queryParameters: query);
}
