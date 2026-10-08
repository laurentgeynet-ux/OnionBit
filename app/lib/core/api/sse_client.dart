// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;

import '../config/app_config.dart';
import '../config/ui_log.dart';
import 'http_transport.dart';

/// Un événement SSE : `event: <topic>` + `data: <json>`.
class SseEvent {
  const SseEvent({required this.topic, required this.data});

  final String topic;
  final Map<String, dynamic> data;

  @override
  String toString() => 'SseEvent($topic)';
}

/// Client SSE pour `GET /api/events` (text/event-stream).
///
/// Équivalent SSE du `RpcClient` WebSocket de l'app de référence :
/// distribution des notifications serveur + reconnexion avec recul
/// exponentiel. Le flux n'est jamais supposé fiable : les listeners
/// doivent se rafraîchir (pull) sur réception plutôt que dépendre de
/// l'ordre des événements.
class SseClient {
  SseClient(this._config, {this.path = '/events', http.Client? httpClient})
    : _http = httpClient ?? createHttpClient();

  final AppConfig _config;

  /// Chemin SSE sous `/api` (`/events` global,
  /// `/messaging/events` messagerie — ADR-0011).
  final String path;
  final http.Client _http;

  StreamSubscription<dynamic>? _sub;
  Timer? _reconnectTimer;
  int _attempt = 0;
  bool _stopped = false;

  final _controller = StreamController<SseEvent>.broadcast();

  /// Flux d'événements parsés (`event:`/`data:`).
  Stream<SseEvent> get events => _controller.stream;

  /// `true` quand le flux SSE est connecté (barre d'état).
  final _connected = StreamController<bool>.broadcast();
  Stream<bool> get connected => _connected.stream;

  /// Démarre la connexion SSE (idempotent).
  void start() {
    _stopped = false;
    _connect();
  }

  void _connect() {
    if (_stopped) return;
    _sub?.cancel();
    final request = http.Request('GET', _config.apiUri(path));
    if (_config.apiKey.isNotEmpty) {
      request.headers['X-Api-Key'] = _config.apiKey;
    }
    _sub = _http
        .send(request)
        .asStream()
        .asyncExpand((resp) {
          if (resp.statusCode != 200) {
            throw ApiSseException(resp.statusCode);
          }
          if (_attempt > 0) {
            uiLog('SSE reconnecte apres $_attempt tentative(s)');
          }
          _attempt = 0;
          _connected.add(true);
          return resp.stream.transform(utf8.decoder);
        })
        .transform(const SseEventParser())
        .listen(
          _controller.add,
          onError: (_) => _scheduleReconnect(),
          onDone: _scheduleReconnect,
          cancelOnError: true,
        );
  }

  void _scheduleReconnect() {
    _connected.add(false);
    _sub?.cancel();
    if (_stopped) return;
    // Recul exponentiel plafonné à 30 s (même politique que
    // l'app de référence).
    final delay = Duration(
      milliseconds: (500 * (1 << _attempt.clamp(0, 6))).clamp(500, 30000),
    );
    _attempt++;
    // Journal : les 5 premieres coupures puis 1 sur 10 (le flux peut
    // osciller longtemps si le daemon est eteint).
    if (_attempt <= 5 || _attempt % 10 == 0) {
      uiLog(
        'SSE coupe (${_config.baseUrl}) — retry dans '
        '${delay.inMilliseconds} ms (tentative $_attempt)',
      );
    }
    _reconnectTimer = Timer(delay, _connect);
  }

  /// Arrête proprement le flux (fermeture de l'app).
  void stop() {
    _stopped = true;
    _reconnectTimer?.cancel();
    _sub?.cancel();
    _connected.add(false);
  }

  void dispose() {
    stop();
    _controller.close();
    _connected.close();
    _http.close();
  }
}

/// Fin de connexion SSE non-200.
class ApiSseException implements Exception {
  ApiSseException(this.statusCode);
  final int statusCode;
  @override
  String toString() => 'ApiSseException($statusCode)';
}

/// Transformeur `text/event-stream` : accumule les lignes jusqu'à une
/// ligne vide, émet un `SseEvent` par trame (`event:` + `data:` JSON).
class SseEventParser extends StreamTransformerBase<String, SseEvent> {
  const SseEventParser();

  @override
  Stream<SseEvent> bind(Stream<String> stream) {
    var event = '';
    final data = StringBuffer();
    final buffer = StringBuffer();
    return stream.asyncExpand((chunk) {
      buffer.write(chunk);
      final lines = buffer.toString().split('\n');
      buffer
        ..clear()
        ..write(lines.removeLast()); // dernier fragment incomplet
      final out = <SseEvent>[];
      for (var line in lines) {
        line = line.endsWith('\r') ? line.substring(0, line.length - 1) : line;
        if (line.isEmpty) {
          if (event.isNotEmpty || data.isNotEmpty) {
            out.add(SseEvent(topic: event, data: _decodeData(data.toString())));
          }
          event = '';
          data.clear();
        } else if (line.startsWith('event:')) {
          event = line.substring(6).trim();
        } else if (line.startsWith('data:')) {
          if (data.isNotEmpty) data.write('\n');
          data.write(line.substring(5).trimLeft());
        }
        // `:` tout seul = keep-alive/commentaire — ignoré.
      }
      return Stream.fromIterable(out);
    });
  }

  static Map<String, dynamic> _decodeData(String raw) {
    if (raw.isEmpty) return {};
    try {
      final v = jsonDecode(raw);
      return v is Map<String, dynamic> ? v : {'value': v};
    } catch (_) {
      return {'raw': raw};
    }
  }
}
