// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/search/domain/search_repository.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
}

/// Dépôt qui échoue toujours `tor_bootstrapping` côté externe —
/// permet de compter les tentatives de `/api/search/external`.
class _BootstrappingRepo implements SearchRepository {
  int externalCalls = 0;

  @override
  Future<List<TorrentResult>> searchLocal(
    String query, {
    String? sortBy,
    bool sortDesc = true,
  }) async => const [];

  @override
  Future<RemoteQuery> searchRemote(String query) async =>
      const RemoteQuery(requestUuid: 'u-test', peers: []);

  @override
  Future<ExternalQuery> searchExternal(String query, String source) {
    externalCalls++;
    throw ApiException(503, 'bootstrap', code: 'tor_bootstrapping');
  }

  @override
  Future<({int seeders, int leechers})?> health(String infohash) async => null;
}

TorRuntimeStatus _status(TorRuntimeState state) => TorRuntimeStatus(
  enabled: true,
  mode: 'embedded',
  state: state,
  progress: state == TorRuntimeState.bootstrapping ? 40 : 100,
  sources: const ['tpb'],
);

void main() {
  /// Lane permanente (ADR-0028) : une recherche externe tombée sur
  /// `tor_bootstrapping` est relancée **automatiquement** quand la
  /// lane passe `ready` — « Entrée » a déjà exprimé l'intention.
  test('relance la recherche externe quand Tor devient ready', () async {
    var torStatus = _status(TorRuntimeState.bootstrapping);
    final repo = _BootstrappingRepo();
    final container = ProviderContainer(
      overrides: [
        sseClientProvider.overrideWithValue(_FakeSseClient()),
        searchRepositoryProvider.overrideWithValue(repo),
        torStatusProvider.overrideWith((ref) async => torStatus),
      ],
    );
    addTearDown(container.dispose);

    // Recherche soumise pendant le bootstrap : une tentative, code
    // `tor_bootstrapping` retenu pour la bannière de progression.
    container.read(searchQueryProvider.notifier).set('ubuntu');
    await container.read(searchResultsProvider.future);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(repo.externalCalls, 1);
    expect(container.read(onionSearchErrorProvider)?.code, 'tor_bootstrapping');

    // La lane devient ready : la même requête repart sans geste.
    torStatus = _status(TorRuntimeState.ready);
    container.refresh(torStatusProvider);
    await container.read(torStatusProvider.future);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(
      repo.externalCalls,
      2,
      reason: 'la transition vers ready doit relancer la recherche',
    );
    // La relance retombe sur le même code (runtime factice) — la
    // bannière reste honnête.
    expect(container.read(onionSearchErrorProvider)?.code, 'tor_bootstrapping');
  });

  /// Pas de relance si l'erreur n'était pas `tor_bootstrapping` —
  /// un défi anti-DDoS ou une panne miroir ne doit pas boucler.
  test('aucune relance sur une autre erreur externe', () async {
    var torStatus = _status(TorRuntimeState.bootstrapping);
    final repo = _BootstrappingRepo();
    final container = ProviderContainer(
      overrides: [
        sseClientProvider.overrideWithValue(_FakeSseClient()),
        searchRepositoryProvider.overrideWithValue(repo),
        torStatusProvider.overrideWith((ref) async => torStatus),
      ],
    );
    addTearDown(container.dispose);

    // Aucune recherche en vol : le passage `ready` ne déclenche rien.
    torStatus = _status(TorRuntimeState.ready);
    await container.read(searchResultsProvider.future);
    container.refresh(torStatusProvider);
    await container.read(torStatusProvider.future);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(repo.externalCalls, 0);
  });
}
