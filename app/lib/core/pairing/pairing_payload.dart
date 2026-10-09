// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Payload du QR d'appairage mobile (ADR-0021 §8, étape 76) :
/// `onionbit://pair/1?host=<h>&port=<p>&token=<hex>` — encodé par le
/// desktop (`PairingSheet`), décodé par le mobile (`qr_scanner`).
/// Le jeton vaut authentification auprès de `POST /api/pairing/
/// redeem` — il n'est jamais persisté, la clé API n'apparaît pas
/// dans le QR.
class PairingPayload {
  const PairingPayload({
    required this.host,
    required this.port,
    required this.token,
  });

  /// Hôte du daemon tel que le mobile le joindra (IP LAN ou nom).
  final String host;

  /// Port HTTP de l'API.
  final int port;

  /// Jeton d'appairage (hex, usage unique — `PairingStore` Rust).
  final String token;

  /// URL de base dérivée pour les appels `redeem` puis de session.
  String get baseUrl => 'http://$host:$port';

  /// Forme QR : `onionbit://pair/1?host=…&port=…&token=…`.
  String encode() => Uri(
    scheme: 'onionbit',
    host: 'pair',
    path: '/1',
    queryParameters: {
      'host': host,
      'port': '$port',
      'token': token,
    },
  ).toString();

  /// `null` sur toute forme non conforme (QR étranger, version
  /// inconnue, port hors bornes, token vide) — l'appelant affiche
  /// l'erreur « QR non reconnu » sans détail exploitable.
  static PairingPayload? tryParse(String raw) {
    final uri = Uri.tryParse(raw.trim());
    if (uri == null ||
        uri.scheme != 'onionbit' ||
        uri.host != 'pair' ||
        uri.pathSegments.length != 1 ||
        uri.pathSegments.first != '1') {
      return null;
    }
    final host = uri.queryParameters['host']?.trim() ?? '';
    final port = int.tryParse(uri.queryParameters['port'] ?? '') ?? 0;
    final token = uri.queryParameters['token']?.trim() ?? '';
    if (host.isEmpty || port <= 0 || port > 65535 || token.isEmpty) {
      return null;
    }
    return PairingPayload(host: host, port: port, token: token);
  }
}
