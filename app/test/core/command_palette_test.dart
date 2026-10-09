// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart' show Override;
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:onionbit_ui/core/command/app_command.dart';
import 'package:onionbit_ui/core/command/command_palette.dart';
import 'package:onionbit_ui/core/shortcuts/app_shortcuts.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_contact.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_conversation.dart';
import 'package:onionbit_ui/features/messaging/presentation/providers/messaging_providers.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';

/// Catalogue + palette de commandes (ADR-0021 §6) : correspondance
/// `matches`, ouverture Ctrl/Cmd+K, filtrage, sélection clavier et
/// exécution (deep-link réglages `/settings?s=`, conversation →
/// `/messages`).
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const conv = MessagingConversation(
    convId: 'aabbccdd00112233',
    kind: 'direct',
    name: '',
    state: 'active',
    createdAt: 1,
    unread: 0,
    lastTs: 0,
    peer: 'deadbeefdeadbeef',
    alias: 'Alice',
  );

  List<Override> overrides() => [
    messagingEnabledProvider.overrideWith((ref) async => true),
    messagingContactsProvider.overrideWith(
      (ref) async => const <MessagingContact>[],
    ),
    messagingConversationsProvider.overrideWith((ref) async => [conv]),
  ];

  /// Routeur minimal : les `run` du catalogue appellent `ctx.go` —
  /// une `MaterialApp` simple ne suffit pas.
  Future<GoRouter> pumpApp(WidgetTester tester) async {
    final router = GoRouter(
      initialLocation: '/',
      routes: [
        GoRoute(
          path: '/',
          builder: (_, _) => const AppShortcuts(
            child: Scaffold(
              body: Focus(autofocus: true, child: SizedBox.expand()),
            ),
          ),
        ),
        for (final p in [
          '/downloads',
          '/search',
          '/messages',
          '/diagnostic',
          '/about',
        ])
          GoRoute(
            path: p,
            builder: (_, _) => Scaffold(body: Text('page $p')),
          ),
        GoRoute(
          path: '/settings',
          builder: (_, state) => Scaffold(
            body: Text('settings ${state.uri.queryParameters['s']}'),
          ),
        ),
      ],
    );
    addTearDown(router.dispose);
    await tester.pumpWidget(
      ProviderScope(
        overrides: overrides(),
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

  Future<void> openPalette(WidgetTester tester) async {
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
  }

  test('AppCommand.matches : titre, sous-titre et mots-clés, casse '
      'insensible', () {
    final cmd = AppCommand(
      id: 't',
      group: CommandGroup.actions,
      icon: Icons.add,
      title: 'Add a download',
      subtitle: 'Settings',
      keywords: 'aimant magnet',
      run: (_, _) {},
    );
    expect(cmd.matches(''), isTrue);
    expect(cmd.matches('download'), isTrue);
    expect(cmd.matches('ADD'), isTrue);
    expect(cmd.matches('settings'), isTrue);
    expect(cmd.matches('aimant'), isTrue); // mot-clé non affiché
    expect(cmd.matches('introuvable'), isFalse);
  });

  testWidgets('Ctrl+K ouvre la palette, Échap la ferme', (tester) async {
    await pumpApp(tester);
    expect(find.byType(CommandPalette), findsNothing);

    await openPalette(tester);
    expect(find.byType(CommandPalette), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
    // Groupes rendus dans l'ordre de `CommandGroup.values` — ceux du
    // bas sont sous le fold de la liste paresseuse : défilement d'abord.
    expect(find.text('NAVIGATION'), findsOneWidget);
    expect(find.text('CONVERSATIONS'), findsOneWidget);
    // La conversation du provider apparaît dans le catalogue.
    expect(find.text('Alice'), findsOneWidget);
    // SETTINGS d'abord (le scroll jusqu'à ACTIONS le ressortirait par
    // le haut), puis le dernier groupe.
    await tester.scrollUntilVisible(
      find.text('SETTINGS'),
      200,
      scrollable: find.byType(Scrollable).last,
    );
    await tester.scrollUntilVisible(
      find.text('ACTIONS'),
      200,
      scrollable: find.byType(Scrollable).last,
    );
    expect(find.text('ACTIONS'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byType(CommandPalette), findsNothing);
  });

  testWidgets('filtrer puis Entrée exécute le deep-link réglages', (
    tester,
  ) async {
    final router = await pumpApp(tester);
    await openPalette(tester);

    await tester.enterText(find.byType(TextField), 'automation');
    await tester.pump();
    expect(find.text('Automation'), findsOneWidget);
    expect(find.text('Alice'), findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(find.byType(CommandPalette), findsNothing);
    expect(router.state.uri.toString(), '/settings?s=automation');
  });

  testWidgets('sélectionner une conversation ouvre son onglet et route '
      'vers /messages', (tester) async {
    final router = await pumpApp(tester);
    await openPalette(tester);

    await tester.enterText(find.byType(TextField), 'alice');
    await tester.pump();
    expect(find.text('Alice'), findsOneWidget);

    // ↓ déplace la sélection (interceptée par `FocusNode.onKeyEvent`),
    // Entrée exécute — vérifie le câblage clavier de la liste.
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(router.state.uri.path, '/messages');
    final ctx = tester.element(find.text('page /messages'));
    expect(
      ProviderScope.containerOf(
        ctx,
      ).read(openConversationsProvider).map((t) => t.convId),
      contains('aabbccdd00112233'),
    );
  });

  testWidgets('requête sans correspondance affiche l\'état vide', (
    tester,
  ) async {
    await pumpApp(tester);
    await openPalette(tester);

    await tester.enterText(find.byType(TextField), 'zzzznope');
    await tester.pump();

    expect(find.text('No matching command'), findsOneWidget);
  });
}
