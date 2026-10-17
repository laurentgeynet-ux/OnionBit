// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/core/layout/top_bar.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';

/// Champ global de la TopBar : aucune requête n'est lancée à la
/// frappe — seule « Entrée » soumet (chaque terme partiel partirait
/// sinon sur les pairs IPv8 et les backends Tor, coût et exposition
/// inutiles).
Widget _host() {
  final router = GoRouter(
    routes: [
      GoRoute(
        path: '/',
        builder: (_, _) => const Scaffold(body: TopBar()),
      ),
      GoRoute(
        path: '/search',
        builder: (_, _) => const Scaffold(body: Text('page recherche')),
      ),
    ],
  );
  addTearDown(router.dispose);
  return MaterialApp.router(
    routerConfig: router,
    localizationsDelegates: AppLocalizations.localizationsDelegates,
    supportedLocales: AppLocalizations.supportedLocales,
    locale: const Locale('en'),
  );
}

void main() {
  testWidgets('frappe sans recherche, Entrée soumet et navigue', (
    tester,
  ) async {
    await tester.pumpWidget(ProviderScope(child: _host()));
    await tester.pump();

    final field = find.byType(TextField);
    expect(field, findsOneWidget);
    final container = ProviderScope.containerOf(
      tester.element(find.byType(TopBar)),
    );

    // Frappe : rien ne part, même après un long délai (le debounce
    // de 300 ms a été retiré — comportement explicite uniquement).
    await tester.enterText(field, 'ubuntu');
    await tester.pump(const Duration(seconds: 1));
    expect(container.read(searchQueryProvider), isEmpty);
    expect(find.text('page recherche'), findsNothing);

    // Entrée : la requête est posée et la page recherche s'ouvre.
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(container.read(searchQueryProvider), 'ubuntu');
    expect(find.text('page recherche'), findsOneWidget);
  });
}
