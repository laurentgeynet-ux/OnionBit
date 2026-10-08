// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/style_guide/style_guide_page.dart';

import 'golden_helpers.dart';

/// Tests golden du design system (ADR-0021 §9/§7) — le guide de style
/// rend tous les tokens/primitifs ; une dérive visuelle future casse
/// une de ces images avant de casser l'œil. Régénérer après un
/// changement de token volontaire : `flutter test --update-goldens`.
void main() {
  testWidgets('guide de style — clair', (tester) async {
    await pumpGolden(
      tester,
      const StyleGuidePage(),
      brightness: Brightness.light,
    );
    await expectLater(
      find.byType(StyleGuidePage),
      matchesGoldenFile('goldens/style_guide_light.png'),
    );
  });

  testWidgets('guide de style — sombre', (tester) async {
    await pumpGolden(
      tester,
      const StyleGuidePage(initialBrightness: Brightness.dark),
      brightness: Brightness.dark,
    );
    await expectLater(
      find.byType(StyleGuidePage),
      matchesGoldenFile('goldens/style_guide_dark.png'),
    );
  });
}
