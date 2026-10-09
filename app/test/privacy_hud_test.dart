// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:onionbit_ui/core/layout/privacy_hud.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';

/// Privacy HUD (ADR-0021 §8) : libellé selon l'état de la lane
/// anonyme (`AnonLaneStatus`), icône seule en compact, tap →
/// `/diagnostic`.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Future<GoRouter> pump(
    WidgetTester tester,
    AnonLaneStatus status, {
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
          anonLaneProvider.overrideWith((ref) async => status),
        ],
        child: MaterialApp.router(
          routerConfig: router,
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
        ),
      ),
    );
    await tester.pumpAndSettle();
    return router;
  }

  testWidgets('prête : circuits et profondeur affichés', (tester) async {
    await pump(
      tester,
      const AnonLaneStatus(
        state: AnonLaneState.ready,
        readyCircuits: 3,
        totalCircuits: 4,
        minReadyHops: 2,
      ),
    );
    expect(find.text('3 circuits · 2 hops'), findsOneWidget);
    expect(find.byIcon(Icons.shield), findsOneWidget);
  });

  testWidgets('en construction et désactivée : libellés dédiés', (
    tester,
  ) async {
    await pump(
      tester,
      const AnonLaneStatus(
        state: AnonLaneState.waiting,
        readyCircuits: 0,
        totalCircuits: 2,
      ),
    );
    expect(find.text('Building circuits…'), findsOneWidget);
  });

  testWidgets('compact : icône seule, pas de libellé', (tester) async {
    await pump(
      tester,
      const AnonLaneStatus(
        state: AnonLaneState.ready,
        readyCircuits: 2,
        totalCircuits: 2,
        minReadyHops: 1,
      ),
      size: const Size(390, 844),
    );
    expect(find.byIcon(Icons.shield), findsOneWidget);
    expect(find.text('2 circuits · 1 hops'), findsNothing);
  });

  testWidgets('tap ouvre le diagnostic', (tester) async {
    final router = await pump(tester, AnonLaneStatus.disabled);
    await tester.tap(find.byType(PrivacyHud));
    await tester.pumpAndSettle();
    expect(router.state.uri.path, '/diagnostic');
  });
}
