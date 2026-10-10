// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../../../core/config/ui_log.dart';
import '../domain/private_zone.dart';

/// Accès REST aux endpoints privés ADR-0027 (`/api/private*`).
class RestPrivateZoneRepository {
  RestPrivateZoneRepository(this._api);

  final ApiClient _api;

  /// Catalogue déchiffré : `state`, entrées manifeste, compteurs
  /// d'orphelins (`GET /api/private`).
  Future<PrivateZoneCatalog> catalog() async {
    final resp = await _api.get('/private') as Map<String, dynamic>;
    final orphans = resp['orphans'] as Map<String, dynamic>? ?? const {};
    return PrivateZoneCatalog(
      state: '${resp['state'] ?? 'locked'}',
      entries: [
        for (final j
            in (resp['downloads'] as List?)?.whereType<
                  Map<String, dynamic>
                >() ??
                const <Map<String, dynamic>>[])
          PrivateEntry(
            infohash: '${j['infohash'] ?? ''}',
            name: '${j['name'] ?? ''}',
            destination: '${j['destination'] ?? ''}',
            paused: j['paused'] == true,
            timeAdded: (j['time_added'] as num?)?.toInt() ?? 0,
          ),
      ],
      orphanGroups: (orphans['obd_groups'] as num?)?.toInt() ?? 0,
      orphanBitv: (orphans['bitv'] as num?)?.toInt() ?? 0,
    );
  }

  /// Fichiers d'une entrée : `{index, path, length}` —
  /// `GET /api/private/{key}/files`.
  Future<List<PrivateFileEntry>> files(String key) async {
    final resp = await _api.get('/private/$key/files') as Map<String, dynamic>;
    return [
      for (final j
          in (resp['files'] as List?)?.whereType<Map<String, dynamic>>() ??
              const <Map<String, dynamic>>[])
        PrivateFileEntry(
          index: (j['index'] as num?)?.toInt() ?? 0,
          path: '${j['path'] ?? ''}',
          length: (j['length'] as num?)?.toInt() ?? 0,
        ),
    ];
  }

  /// Copie déchiffrée vers `destDir` (`POST /api/private/{key}/export`)
  /// — `files` = sous-ensemble d'index (`null` = tout).
  Future<PrivateExportResult> export(
    String key,
    String destDir, {
    List<int>? files,
  }) async {
    // Ni la cle ni les noms : metadonnees sensibles (ADR-0027 §2).
    uiLog('export prive -> $destDir');
    final resp =
        await _api.post('/private/$key/export', body: {
              'dest_dir': destDir,
              'files': ?files,
            })
            as Map<String, dynamic>;
    return PrivateExportResult(
      exported: (resp['exported'] as num?)?.toInt() ?? 0,
      bytes: (resp['bytes'] as num?)?.toInt() ?? 0,
    );
  }
}
