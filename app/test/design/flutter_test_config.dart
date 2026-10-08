// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/services.dart' show FontLoader, rootBundle;
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/tokens/typography_tokens.dart';

/// Config globale des tests — convention `package:flutter_test` :
/// chargée automatiquement pour tout `flutter test` sous `app/test/`.
///
/// Charge les trois polices maison une seule fois pour que les tests
/// golden (`test/design/`) rendent les vraies fontes plutôt que la
/// police de repli « Ahem » (carrés) — ADR-0021 §9. Coût négligeable
/// pour les tests non golden.
Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  setUpAll(() async {
    Future<void> load(String family, String asset) async {
      final loader = FontLoader(family)..addFont(rootBundle.load(asset));
      await loader.load();
    }

    await load(
      AppFontFamilies.display,
      'assets/fonts/SpaceGrotesk/SpaceGrotesk-Variable.ttf',
    );
    await load(AppFontFamilies.body, 'assets/fonts/Inter/Inter-Variable.ttf');
    await load(
      AppFontFamilies.mono,
      'assets/fonts/JetBrainsMono/JetBrainsMono-Variable.ttf',
    );
  });

  await testMain();
}
