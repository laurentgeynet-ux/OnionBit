// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../domain/settings_repository.dart';

/// Implémentation REST du dépôt réglages.
class RestSettingsRepository implements SettingsRepository {
  RestSettingsRepository(this._api);

  final ApiClient _api;

  @override
  Future<Map<String, dynamic>> get() async =>
      (await _api.get('/settings') as Map<String, dynamic>)['settings']
          as Map<String, dynamic>? ??
      const {};

  @override
  Future<void> update(Map<String, dynamic> settings) =>
      _api.post('/settings', body: {'settings': settings});

  @override
  Future<void> shutdown() => _api.put('/shutdown');

  @override
  Future<Map<String, int>> dirSpace({String? directory, String? area}) async {
    final resp = await _api.put(
      '/statistics/dirspace',
      body: {'directory': ?directory, 'area': ?area},
    ) as Map<String, dynamic>;
    final s = resp['statistics'] as Map<String, dynamic>? ?? const {};
    return {
      'total': (s['total'] as num?)?.toInt() ?? 0,
      'used': (s['used'] as num?)?.toInt() ?? 0,
      'free': (s['free'] as num?)?.toInt() ?? 0,
    };
  }

  @override
  Future<List<Map<String, dynamic>>> rssItems() async {
    final resp = await _api.get('/rss') as Map<String, dynamic>;
    return (resp['items'] as List?)
            ?.whereType<Map<String, dynamic>>()
            .toList() ??
        const [];
  }

  @override
  Future<void> setRssFeeds(List<String> urls) =>
      _api.put('/rss', body: {'urls': urls});

  @override
  Future<Map<String, dynamic>> versions() async =>
      await _api.get('/versioning/versions') as Map<String, dynamic>? ??
      const {};

  @override
  Future<Map<String, dynamic>> checkVersion() async =>
      await _api.get('/versioning/versions/check') as Map<String, dynamic>? ??
      const {};

  @override
  Future<String?> identityPublicKey() async {
    try {
      final resp = await identityStatus();
      return resp['public_key'] as String?;
    } catch (_) {
      return null; // IPv8 desactive — pas d'identite a afficher.
    }
  }

  @override
  Future<Map<String, dynamic>> identityStatus() async =>
      await _api.get('/identity') as Map<String, dynamic>? ?? const {};

  @override
  Future<String?> identityRecoveryPhrase({String? lang}) async {
    try {
      final resp =
          await _api.get(
                '/identity/recovery_phrase',
                query: {'lang': ?lang},
              )
              as Map<String, dynamic>?;
      return resp?['phrase'] as String?;
    } catch (_) {
      // Identite legacy (404) ou indisponible : pas de phrase.
      return null;
    }
  }

  @override
  Future<Map<String, dynamic>> identityExport({String? password}) async =>
      await _api.post(
            '/identity/export',
            body: {
              if (password != null && password.isNotEmpty)
                'password': password,
            },
          )
          as Map<String, dynamic>? ??
      const {};

  @override
  Future<void> identityRestore(
    String keyHex, {
    String? password,
    bool forceLegacy = false,
  }) =>
      _api.post(
        '/identity/restore',
        body: {
          'key': keyHex,
          if (password != null && password.isNotEmpty)
            'password': password,
          if (forceLegacy) 'force_legacy': true,
        },
      );

  @override
  Future<void> identityRestorePhrase(String phrase) =>
      _api.post('/identity/restore', body: {'phrase': phrase});

  @override
  Future<void> identitySetAtRest({
    required bool enabled,
    required String password,
  }) =>
      _api.post(
        '/identity/at_rest',
        body: {'enabled': enabled, 'password': password},
      );

  @override
  Future<void> identityCreate({String? password}) => _api.post(
    '/identity/create',
    body: {
      if (password != null && password.isNotEmpty) 'password': password,
    },
  );

  @override
  Future<void> identityGuest() => _api.post('/identity/guest');

  @override
  Future<void> identityUnlock(String password) =>
      _api.post('/identity/unlock', body: {'password': password});

  @override
  Future<Map<String, dynamic>> privateZone() async =>
      await _api.get('/private') as Map<String, dynamic>? ?? const {};

  @override
  Future<void> purgePrivateOrphans() => _api.delete('/private/orphans');
}
