// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/core/layout/privacy_hud.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
}

/// Bandeau « état du réseau » (4 segments : daemon, overlay,
/// tunnels, Tor) — icônes colorées + tooltips, tap → `/diagnostic`.
/// « Désactivé » s'affiche en gris, jamais comme une panne.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Future<GoRouter> pump(
    WidgetTester tester, {
    AnonLaneStatus lane = AnonLaneStatus.disabled,
    TorRuntimeStatus tor = TorRuntimeStatus.disabled,
    List<OverlayInfo> overlays = const [],
    bool connected = true,
    Size size = const Size(1280, 800),
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final router = GoRouter(
      initialLocation: '/',
      routes: [
        GoRoute(
          path: '/',
          builder: (_, _) =>
              const Scaffold(body: Center(child: PrivacyHud())),
        ),
        GoRoute(
          path: '/diagnostic',
          builder: (_, _) => const Scaffold(body: Text('diagnostic')),
        ),
      ],
    );
    addTearDown(router.dispose);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          sseClientProvider.overrideWithValue(_FakeSseClient()),
          sseConnectedProvider.overrideWith(
            (ref) => Stream.value(connected),
          ),
          anonLaneProvider.overrideWith((ref) async => lane),
          overlaysProvider.overrideWith((ref) async => overlays),
          torStatusProvider.overrideWith((ref) async => tor),
        ],
        child: MaterialApp.router(
          routerConfig: router,
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          locale: const Locale('en'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    return router;
  }

  String tooltipOf(WidgetTester tester, String needle) {
    for (final t in tester.widgetList<Tooltip>(find.byType(Tooltip))) {
      if ('${t.message}'.contains(needle)) return '${t.message}';
    }
    throw StateError('tooltip « $needle » absent');
  }

  testWidgets('4 segments présents — daemon, overlay, tunnels, Tor', (
    tester,
  ) async {
    await pump(
      tester,
      lane: const AnonLaneStatus(
        state: AnonLaneState.ready,
        readyCircuits: 3,
        totalCircuits: 4,
        minReadyHops: 2,
      ),
      overlays: const [
        OverlayInfo(name: 'discovery', id: 'a', peers: 5),
        OverlayInfo(name: 'tunnel', id: 'b', peers: 3),
      ],
    );
    expect(find.byIcon(Icons.dns_outlined), findsOneWidget);
    expect(find.byIcon(Icons.hub_outlined), findsOneWidget);
    expect(find.byIcon(Icons.shield), findsOneWidget);
    expect(find.byIcon(Icons.vpn_lock_outlined), findsOneWidget);
    // Tooltips : détails réels par segment.
    expect(
      tooltipOf(tester, 'Daemon connected'),
      contains('SSE'),
    );
    expect(tooltipOf(tester, 'peers'), contains('8'));
    expect(tooltipOf(tester, 'circuits'), contains('3'));
  });

  testWidgets('Tor désactivé : segment grisé, jamais en panne', (
    tester,
  ) async {
    await pump(tester);
    expect(tooltipOf(tester, 'Tor disabled'), contains('Settings'));
  });

  testWidgets('Tor ready : segment vert « lane prête »', (tester) async {
    await pump(
      tester,
      tor: const TorRuntimeStatus(
        enabled: true,
        mode: 'embedded',
        state: TorRuntimeState.ready,
        progress: 100,
      ),
    );
    expect(find.byIcon(Icons.vpn_lock), findsOneWidget);
    expect(tooltipOf(tester, 'lane ready'), contains('Tor'));
  });

  testWidgets('daemon coupé : segment rouge « unreachable »', (
    tester,
  ) async {
    await pump(tester, connected: false);
    expect(tooltipOf(tester, 'unreachable'), isNotEmpty);
  });

  testWidgets('tap ouvre le diagnostic', (tester) async {
    final router = await pump(tester);
    await tester.tap(find.byType(PrivacyHud));
    await tester.pumpAndSettle();
    expect(router.state.uri.path, '/diagnostic');
  });
}
