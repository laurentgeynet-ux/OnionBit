import 'dart:convert';

import 'package:http/http.dart' as http;

import '../config/app_config.dart';

/// Erreur API au format Tribler : `{"error": {"handled": b, "message": m}}`.
class ApiException implements Exception {
  ApiException(this.statusCode, this.message, {this.handled = true});

  final int statusCode;
  final String message;
  final bool handled;

  @override
  String toString() => 'ApiException($statusCode): $message';
}

/// Client REST de `tribler-api`.
///
/// Toutes les features passent par cette classe (comme `RpcClient`
/// dans l'app de référence) : encodage JSON, `X-Api-Key`, décodage des
/// erreurs au format Python `{"error": {"handled","message"}}`.
class ApiClient {
  ApiClient(this._config, {http.Client? httpClient})
    : _http = httpClient ?? http.Client();

  final AppConfig _config;
  final http.Client _http;

  Map<String, String> get _headers => {
    'Content-Type': 'application/json',
    if (_config.apiKey.isNotEmpty) 'X-Api-Key': _config.apiKey,
  };

  Future<dynamic> get(String path, {Map<String, String>? query}) =>
      _send((uri) => _http.get(uri, headers: _headers), path, query);

  /// `GET` d'une réponse en texte brut (`/api/logging` n'est pas JSON).
  Future<String> getText(String path, {Map<String, String>? query}) async {
    final resp = await _http.get(
      _config.apiUri(path, query),
      headers: _headers,
    );
    if (resp.statusCode >= 400) throw _decodeError(resp);
    return resp.body;
  }

  Future<dynamic> put(
    String path, {
    Object? body,
    Map<String, String>? query,
  }) => _send(
    (uri) => _http.put(uri, headers: _headers, body: jsonEncode(body)),
    path,
    query,
  );

  /// `PUT` d'un `.torrent` brut (`Content-Type: applications/x-bittorrent`,
  /// paramètres en query — contrat Python, fonctionne aussi sur web).
  Future<dynamic> putTorrent(
    String path,
    List<int> bytes, {
    Map<String, String>? query,
  }) => _send(
    (uri) => _http.put(
      uri,
      headers: {..._headers, 'Content-Type': 'applications/x-bittorrent'},
      body: bytes,
    ),
    path,
    query,
  );

  Future<dynamic> post(String path, {Object? body}) => _send(
    (uri) => _http.post(uri, headers: _headers, body: jsonEncode(body)),
    path,
    null,
  );

  Future<dynamic> patch(String path, {Object? body}) => _send(
    (uri) => _http.patch(uri, headers: _headers, body: jsonEncode(body)),
    path,
    null,
  );

  Future<dynamic> delete(String path, {Object? body}) => _send(
    (uri) => _http.delete(uri, headers: _headers, body: jsonEncode(body)),
    path,
    null,
  );

  Future<dynamic> _send(
    Future<http.Response> Function(Uri uri) call,
    String path,
    Map<String, String>? query,
  ) async {
    final resp = await call(_config.apiUri(path, query));
    if (resp.statusCode >= 400) {
      throw _decodeError(resp);
    }
    if (resp.body.isEmpty) return null;
    return jsonDecode(resp.body);
  }

  ApiException _decodeError(http.Response resp) {
    try {
      final json = jsonDecode(resp.body);
      final err = json['error'];
      if (err is Map) {
        return ApiException(
          resp.statusCode,
          '${err['message']}',
          handled: err['handled'] == true,
        );
      }
    } catch (_) {
      // Corps non-JSON : on retombe sur le corps brut.
    }
    return ApiException(resp.statusCode, resp.body, handled: false);
  }

  void close() => _http.close();
}
