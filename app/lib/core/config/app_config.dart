// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Configuration de connexion au daemon `onionbit-daemon`.
///
/// V1 : daemon embarqué ou local, API sur `127.0.0.1:8085` (défaut
/// historique). `apiKey` correspond à `api.key` du
/// `configuration.json` du daemon — exigée même sur loopback (parité
/// `ApiKeyMiddleware` Tribler) ; elle est résolue automatiquement par
/// `daemon_api_resolver` quand le fichier est accessible.
class AppConfig {
  const AppConfig({this.baseUrl = 'http://127.0.0.1:8085', this.apiKey = ''});

  /// URL de base de `onionbit-api` (schéma + hôte + port, sans `/api`).
  final String baseUrl;

  /// Clé `X-Api-Key` si le daemon en exige une.
  final String apiKey;

  /// URI complète d'un chemin d'API (`/downloads`, …).
  Uri apiUri(String path, [Map<String, String>? query]) =>
      Uri.parse('$baseUrl/api$path').replace(queryParameters: query);
}
