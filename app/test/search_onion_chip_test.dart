// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/pages/search_page.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'helpers/l10n.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
}

class _EmptySearch extends SearchResultsNotifier {
  @override
  Future<List<TorrentResult>> build() async => [];
}

class _EmptyRemote extends RemoteResultsNotifier {
  @override
  RemoteResults build() => const RemoteResults(
    state: RemoteSearchState(uuid: null, peerCount: 0),
    results: [],
  );
}

const _ready = TorRuntimeStatus(
  enabled: true,
  mode: 'embedded',
  state: TorRuntimeState.ready,
  progress: 100,
  sources: ['tpb'],
);

TorSourceProbe _probe(int reachable) => TorSourceProbe(
  source: 'tpb',
  reachable: reachable,
  mirrors: [
    TorMirrorProbe(
      host: 'miroir.onion',
      ok: reachable > 0,
      status: reachable > 0 ? 200 : null,
      latencyMs: 1200,
      error: reachable > 0 ? null : 'unreachable',
    ),
  ],
);

Widget _app({required int reachable}) {
  return ProviderScope(
    overrides: [
      sseClientProvider.overrideWithValue(_FakeSseClient()),
      searchResultsProvider.overrideWith(_EmptySearch.new),
      remoteResultsProvider.overrideWith(_EmptyRemote.new),
      torStatusProvider.overrideWith((ref) async => _ready),
      torSourceProbeProvider.overrideWith(
        (ref, arg) async => _probe(reachable),
      ),
    ],
    child: l10nTestApp(const Scaffold(body: SearchPage())),
  );
}

/// Message du `Tooltip` de la pastille santé du chip onion.
String? _probeTooltip(WidgetTester tester) {
  for (final t in tester.widgetList<Tooltip>(find.byType(Tooltip))) {
    final m = '${t.message}';
    if (m.contains('mirror')) return m;
  }
  return null;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('pastille verte : miroir joignable sans recherche lancée', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(_app(reachable: 1));
    await tester.pumpAndSettle();

    expect(_probeTooltip(tester), contains('1/1 onion mirror'));
  });

  testWidgets('pastille rouge : onion tombé signalé (déprécié)', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(_app(reachable: 0));
    await tester.pumpAndSettle();

    expect(_probeTooltip(tester), contains('No onion mirror reachable'));
  });
}
