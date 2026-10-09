// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../domain/privacy_profile.dart';
import '../domain/privacy_repository.dart';

/// Implémentation REST du dépôt profil d'anonymat.
class RestPrivacyRepository implements PrivacyRepository {
  RestPrivacyRepository(this._api);

  final ApiClient _api;

  @override
  Future<PrivacyProfileState> profile() async => PrivacyProfileState.fromJson(
    await _api.get('/privacy/profile') as Map<String, dynamic>? ?? const {},
  );

  @override
  Future<PrivacySwitchResult> switchProfile(
    PrivacyProfileKind target,
  ) async {
    final resp =
        await _api.put('/privacy/profile', body: {'profile': target.wire})
            as Map<String, dynamic>? ??
        const {};
    return PrivacySwitchResult(
      modified: resp['modified'] == true,
      restartRequired: resp['restart_required'] == true,
    );
  }

  @override
  Future<void> addBridge(String link) =>
      _api.post('/stealth/bridges', body: {'link': link});

  @override
  Future<void> shutdown() => _api.put('/shutdown');
}
