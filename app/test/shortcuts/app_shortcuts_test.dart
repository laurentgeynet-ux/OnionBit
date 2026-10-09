// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/shortcuts/app_intents.dart';
import 'package:onionbit_ui/core/shortcuts/app_shortcuts.dart';

/// Vérifie le mappage touches → intents d'`AppShortcuts` (ADR-0021
/// §6). Un `Actions` de test plus profond que celui d'`AppShortcuts`
/// intercepte l'intent (résolution « au plus proche » du framework) —
/// on teste le câblage clavier, pas les effets par défaut
/// (`context.go`/dialogue), qui dépendent d'un routeur/BuildContext
/// réels hors du périmètre de ce test unitaire.
void main() {
  Future<void> pumpWithSpy(
    WidgetTester tester,
    Map<Type, Action<Intent>> spyActions,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: AppShortcuts(
          child: Actions(
            actions: spyActions,
            child: const Focus(autofocus: true, child: SizedBox.expand()),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('Ctrl+F déclenche GoToSearchIntent', (tester) async {
    var invoked = false;
    await pumpWithSpy(tester, {
      GoToSearchIntent: CallbackAction<GoToSearchIntent>(
        onInvoke: (intent) {
          invoked = true;
          return null;
        },
      ),
    });

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();

    expect(invoked, isTrue);
  });

  testWidgets('Ctrl+N déclenche AddDownloadIntent', (tester) async {
    var invoked = false;
    await pumpWithSpy(tester, {
      AddDownloadIntent: CallbackAction<AddDownloadIntent>(
        onInvoke: (intent) {
          invoked = true;
          return null;
        },
      ),
    });

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyN);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();

    expect(invoked, isTrue);
  });

  testWidgets('Cmd (meta) déclenche aussi les intents (macOS)', (
    tester,
  ) async {
    var invoked = false;
    await pumpWithSpy(tester, {
      GoToSearchIntent: CallbackAction<GoToSearchIntent>(
        onInvoke: (intent) {
          invoked = true;
          return null;
        },
      ),
    });

    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await tester.pump();

    expect(invoked, isTrue);
  });
}
