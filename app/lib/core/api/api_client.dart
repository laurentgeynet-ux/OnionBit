// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';

import 'package:http/http.dart' as http;

import '../config/app_config.dart';
import 'http_transport.dart';

/// Erreur API au format Tribler : `{"error": {"handled": b, "message": m}}`.
class ApiException implements Exception {
  ApiException(this.statusCode, this.message, {this.handled = true});

  final int statusCode;
  final String message;
  final bool handled;

  @override
  String toString() => 'ApiException($statusCode): $message';
}

/// Le daemon ne repond pas (eteint, port ferme, refus de connexion) —
/// pas une erreur HTTP mais un echec de transport. `http.ClientException`
/// enveloppe `SocketException` cote natif et le `TypeError` fetch cote
/// web : ce type unique permet a l'UI d'afficher un message clair au
/// lieu de la trace brute.
class DaemonUnreachableException implements Exception {
  DaemonUnreachableException(this.uri, this.cause);

  final Uri uri;
  final Object cause;

  @override
  String toString() => 'DaemonUnreachableException: $uri ($cause)';
}

/// Client REST de `onionbit-api`.
///
/// Toutes les features passent par cette classe (comme `RpcClient`
/// dans l'app de référence) : encodage JSON, `X-Api-Key`, décodage des
/// erreurs au format Python `{"error": {"handled","message"}}`.
class ApiClient {
  ApiClient(this._config, {http.Client? httpClient})
    : _http = httpClient ?? createHttpClient();

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
    final uri = _config.apiUri(path, query);
    final http.Response resp;
    try {
      resp = await _http.get(uri, headers: _headers);
    } on http.ClientException catch (e) {
      throw DaemonUnreachableException(uri, e);
    }
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

  /// `POST` d'octets bruts (`Content-Type: application/octet-stream`,
  /// nom en query — `POST /messaging/uploads?name=`).
  Future<dynamic> postBytes(
    String path,
    List<int> bytes, {
    Map<String, String>? query,
  }) => _send(
    (uri) => _http.post(
      uri,
      headers: {..._headers, 'Content-Type': 'application/octet-stream'},
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

  /// `GET` d'une réponse en flux de lignes (`text/event-stream` ou
  /// texte découpé). Utilisé par le speed test de circuit
  /// (`speed: {json}` par ligne — format pyipv8, sans `data:` SSE).
  ///
  /// En cas d'erreur HTTP, le corps est lu en entier : le endpoint
  /// tunnel renvoie `{"error": "msg"}` brut (sans enveloppe
  /// `{"error": {"handled", "message"}}` — forme `Response(dict)` de
  /// pyipv8).
  Stream<String> getStreamedLines(
    String path, {
    Map<String, String>? query,
  }) async* {
    final uri = _config.apiUri(path, query);
    final request = http.Request('GET', uri);
    request.headers.addAll(_headers);
    final http.StreamedResponse resp;
    try {
      resp = await _http.send(request);
    } on http.ClientException catch (e) {
      throw DaemonUnreachableException(uri, e);
    }
    if (resp.statusCode >= 400) {
      final body = await resp.stream.bytesToString();
      var message = body;
      try {
        final json = jsonDecode(body);
        if (json is Map) {
          final err = json['error'];
          if (err is String) {
            message = err;
          } else if (err is Map) {
            message = '${err['message']}';
          }
        }
      } catch (_) {
        // Corps non-JSON : message brut.
      }
      throw ApiException(resp.statusCode, message);
    }
    yield* resp.stream.transform(utf8.decoder).transform(const LineSplitter());
  }

  Future<dynamic> _send(
    Future<http.Response> Function(Uri uri) call,
    String path,
    Map<String, String>? query,
  ) async {
    final uri = _config.apiUri(path, query);
    final http.Response resp;
    try {
      resp = await call(uri);
    } on http.ClientException catch (e) {
      throw DaemonUnreachableException(uri, e);
    }
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
