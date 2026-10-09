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

/// Audit d'accessibilité automatisé (ADR-0021 §9, étape 77) — ce que
/// les tests peuvent garantir sans lecteur d'écran : les indicateurs
/// icon-only portent un `Semantics` label (sinon ils sont invisibles
/// pour TalkBack/VoiceOver/NVDA), et le libellé reflète l'état réel.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  /// Largeur `compact` = pire cas : le HUD n'y montre que l'icône —
  /// le `Semantics` est alors la seule information accessible.
  Future<void> pumpHud(WidgetTester tester, AnonLaneStatus lane) async {
    tester.view.physicalSize = const Size(390, 200);
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
      ],
    );
    addTearDown(router.dispose);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          anonLaneProvider.overrideWith((ref) async => lane),
        ],
        child: MaterialApp.router(
          routerConfig: router,
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('HUD désactivé : bouton sémantique étiqueté', (
    tester,
  ) async {
    await pumpHud(tester, AnonLaneStatus.disabled);
    // Le label fusionne l'état + l'infobulle (« … — … diagnostics »).
    expect(
      find.bySemanticsLabel(RegExp('Anonymity off')),
      findsOneWidget,
    );
    expect(find.byIcon(Icons.shield_outlined), findsOneWidget);
  });

  testWidgets('HUD prêt : le label annonce circuits et sauts', (
    tester,
  ) async {
    await pumpHud(
      tester,
      const AnonLaneStatus(
        state: AnonLaneState.ready,
        readyCircuits: 3,
        totalCircuits: 3,
        minReadyHops: 2,
      ),
    );
    expect(find.bySemanticsLabel(RegExp('3 circuits')), findsOneWidget);
  });
}
