// This file is part of OnionBit - a Rust port of the Tribler daemon.
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
  Future<Map<String, int>> dirSpace({String? directory}) async {
    final resp = await _api.put(
      '/statistics/dirspace',
      body: {'directory': ?directory},
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
}
