// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:http/http.dart' as http;

import '../api/api_client.dart';
import '../config/app_config.dart';

/// Grant d'appairage émis par `POST /api/pairing/token` (ADR-0021
/// §8) — jeton à usage unique + TTL court côté daemon.
class PairingGrant {
  const PairingGrant({required this.token, required this.expiresInSecs});

  final String token;
  final int expiresInSecs;
}

/// Façade des deux appels d'appairage mobile : `issue` (daemon
/// configuré, authentifié — le desktop qui affiche le QR) et
/// `redeem` (daemon distant découvert via le QR — l'appel part
/// **sans** clé, c'est le jeton qui authentifie ; voir `auth.rs`).
class PairingApi {
  const PairingApi();

  /// `POST /api/pairing/token` sur le daemon configuré — réutilise
  /// le client authentifié existant (`apiClientProvider`).
  Future<PairingGrant> issue(ApiClient api) async {
    final resp = await api.post('/pairing/token');
    return PairingGrant(
      token: '${resp['token']}',
      expiresInSecs: (resp['expires_in_secs'] as num?)?.toInt() ?? 0,
    );
  }

  /// `POST /api/pairing/redeem` sur `baseUrl` (décodé du QR) — sans
  /// clé API ; rend la clé du daemon en cas de succès. Jeton
  /// inconnu/expiré/consommé → 401 (`ApiException`), daemon
  /// injoignable → `DaemonUnreachableException`. `httpClient` n'est
  /// exposé que pour les tests (`MockClient`).
  Future<String> redeem(
    String baseUrl,
    String token, {
    http.Client? httpClient,
  }) async {
    final api = ApiClient(
      AppConfig(baseUrl: baseUrl),
      httpClient: httpClient,
    );
    try {
      final resp = await api.post(
        '/pairing/redeem',
        body: {'token': token},
      );
      return '${resp['api_key']}';
    } finally {
      api.close();
    }
  }
}
