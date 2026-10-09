// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart' show MaterialApp, NavigationBar, Size;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/app.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/design/design_tokens.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/core/layout/app_sidebar.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/downloads/domain/download.dart';
import 'package:onionbit_ui/features/downloads/presentation/providers/downloads_providers.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
}

class _EmptyDownloads extends DownloadsNotifier {
  @override
  Future<List<Download>> build() async => [];
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

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('le shell affiche la sidebar et la page téléchargements', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});

    // Largeur explicite (palier `expanded`, ADR-0021 §5) : la taille
    // par défaut de `flutter test` (800x600) tombe dans `medium`, où
    // la sidebar est désormais volontairement repliée (icônes seules,
    // AppShell/AppSidebar) — ce test vérifie le rendu « libellés
    // visibles », pas le comportement `medium`, d'où la taille fixée.
    tester.view.physicalSize = const Size(1280, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          sseClientProvider.overrideWithValue(_FakeSseClient()),
          downloadsProvider.overrideWith(_EmptyDownloads.new),
          searchResultsProvider.overrideWith(_EmptySearch.new),
          remoteResultsProvider.overrideWith(_EmptyRemote.new),
          anonLaneProvider.overrideWith((ref) async => AnonLaneStatus.disabled),
        ],
        child: const OnionbitApp(),
      ),
    );
    await tester.pump();
    await tester.pump();

    expect(find.text('Add'), findsWidgets);
    expect(find.text('Downloads'), findsWidgets);
    expect(find.text('Search'), findsOneWidget);
    expect(find.text('Diagnostics'), findsOneWidget);
    expect(find.text('No downloads'), findsOneWidget);

    // Étape 73 : le thème applicatif est `AppDesignTheme` — extension
    // sémantique enregistrée et typographie de marque (Inter en corps,
    // Space Grotesk en titres) actives, pas le thème hérité.
    final app = tester.widget<MaterialApp>(find.byType(MaterialApp));
    final theme = app.theme!;
    expect(theme.extension<AppSemanticColorsExtension>(), isNotNull);
    expect(theme.textTheme.bodyMedium?.fontFamily, AppFontFamilies.body);
    expect(theme.textTheme.titleLarge?.fontFamily, AppFontFamilies.display);
    final dark = app.darkTheme!;
    expect(dark.extension<AppSemanticColorsExtension>(), isNotNull);
  });

  // ADR-0021 §5 : la coquille distingue désormais les quatre paliers
  // de `AppBreakpoints` au lieu de seulement compact/non-compact.
  testWidgets('coquille compacte : barre de navigation basse, pas de sidebar', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(400, 800); // < 600 dp = compact
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          sseClientProvider.overrideWithValue(_FakeSseClient()),
          downloadsProvider.overrideWith(_EmptyDownloads.new),
          searchResultsProvider.overrideWith(_EmptySearch.new),
          remoteResultsProvider.overrideWith(_EmptyRemote.new),
          anonLaneProvider.overrideWith((ref) async => AnonLaneStatus.disabled),
        ],
        child: const OnionbitApp(),
      ),
    );
    await tester.pump();
    await tester.pump();

    expect(find.byType(NavigationBar), findsOneWidget);
    // `Search` reste visible (libellé de la nav basse) — c'est
    // l'absence de la sidebar elle-même qui distingue `compact`.
    expect(find.text('Search'), findsOneWidget);
    expect(find.byType(AppSidebar), findsNothing);
  });

  testWidgets('coquille medium : sidebar forcée en rail (icônes seules)', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(800, 700); // 600-1024 dp = medium
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          sseClientProvider.overrideWithValue(_FakeSseClient()),
          downloadsProvider.overrideWith(_EmptyDownloads.new),
          searchResultsProvider.overrideWith(_EmptySearch.new),
          remoteResultsProvider.overrideWith(_EmptyRemote.new),
          anonLaneProvider.overrideWith((ref) async => AnonLaneStatus.disabled),
        ],
        child: const OnionbitApp(),
      ),
    );
    await tester.pump();
    await tester.pump();

    // Pas de barre basse (on n'est pas en compact) ...
    expect(find.byType(NavigationBar), findsNothing);
    expect(find.byType(AppSidebar), findsOneWidget);
    // ... mais le rail est replié : le libellé « Search » de la
    // sidebar disparaît (icône + tooltip seulement, ADR-0021 §5 —
    // `collapsedOverride: true`). `Downloads` reste trouvable
    // ailleurs dans l'écran (contenu de la page), donc non testé ici.
    expect(find.text('Search'), findsNothing);
    expect(find.byTooltip('Search'), findsOneWidget);
  });
}
