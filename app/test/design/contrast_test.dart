// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/design_tokens.dart';

/// Porte de contraste WCAG AA (ADR-0021 §9) — vérifie, pour les deux
/// thèmes, que chaque paire de tokens effectivement utilisée comme
/// « texte sur fond » atteint le ratio 4.5:1 du texte courant.
///
/// Paires volontairement exclues : `outline` et `tertiary` sur
/// `surface` — le cyan de marque sert d'accent décoratif (icônes,
/// pastilles, liserés), jamais de couleur de texte ; sa paire porteuse
/// (`onTertiary`/`tertiary`, `onTertiaryContainer`/`tertiaryContainer`)
/// est testée.
void main() {
  // Le `setUpAll` global (`flutter_test_config.dart`) charge les
  // polices via `rootBundle` — il exige le binding même pour des
  // tests purement calculatoires comme celui-ci.
  TestWidgetsFlutterBinding.ensureInitialized();

  // Luminance relative WCAG : canal sRGB → linéaire puis pondération
  // 0.2126/0.7152/0.0722 ; ratio = (Lclair + .05)/(Lsombre + .05).
  double lum(Color c) {
    double chan(double v) {
      final s = v / 255;
      return s <= 0.04045
          ? s / 12.92
          : math.pow((s + 0.055) / 1.055, 2.4).toDouble();
    }

    return 0.2126 * chan(c.r * 255) +
        0.7152 * chan(c.g * 255) +
        0.0722 * chan(c.b * 255);
  }

  double ratio(Color a, Color b) {
    final la = lum(a), lb = lum(b);
    final hi = math.max(la, lb), lo = math.min(la, lb);
    return (hi + 0.05) / (lo + 0.05);
  }

  /// Assert sur une paire « texte » : ≥ 4.5 (AA texte courant).
  void check(String name, Color fg, Color bg, List<String> failures) {
    final r = ratio(fg, bg);
    if (r < 4.5) {
      failures.add('$name : ${r.toStringAsFixed(2)}:1 (< 4.5)');
    }
  }

  for (final (label, theme) in [
    ('clair', AppDesignTheme.light()),
    ('sombre', AppDesignTheme.dark()),
  ]) {
    test('thème $label : paires texte/fond conformes WCAG AA', () {
      final cs = theme.colorScheme;
      final sem = theme.extension<AppSemanticColorsExtension>()!.colors;
      final failures = <String>[];

      // Corps et texte secondaire sur surfaces M3 réellement utilisées.
      check('onSurface/surface', cs.onSurface, cs.surface, failures);
      check(
        'onSurface/containerHighest',
        cs.onSurface,
        cs.surfaceContainerHighest,
        failures,
      );
      check(
        'onSurfaceVariant/surface',
        cs.onSurfaceVariant,
        cs.surface,
        failures,
      );
      check(
        'onSurfaceVariant/containerHighest',
        cs.onSurfaceVariant,
        cs.surfaceContainerHighest,
        failures,
      );

      // Rôles Material : texte sur conteneur teinté + « on » sur rôle.
      check('primary/surface', cs.primary, cs.surface, failures);
      check('onPrimary/primary', cs.onPrimary, cs.primary, failures);
      check(
        'onPrimaryContainer/primaryContainer',
        cs.onPrimaryContainer,
        cs.primaryContainer,
        failures,
      );
      check(
        'onSecondaryContainer/secondaryContainer',
        cs.onSecondaryContainer,
        cs.secondaryContainer,
        failures,
      );
      check('onTertiary/tertiary', cs.onTertiary, cs.tertiary, failures);
      check(
        'onTertiaryContainer/tertiaryContainer',
        cs.onTertiaryContainer,
        cs.tertiaryContainer,
        failures,
      );
      check('error/surface', cs.error, cs.surface, failures);
      check('onError/error', cs.onError, cs.error, failures);
      check(
        'onErrorContainer/errorContainer',
        cs.onErrorContainer,
        cs.errorContainer,
        failures,
      );

      // Tokens sémantiques du design system (succès/warning/info).
      check('onSuccess/success', sem.onSuccess, sem.success, failures);
      check(
        'onSuccessContainer/successContainer',
        sem.onSuccessContainer,
        sem.successContainer,
        failures,
      );
      check('onWarning/warning', sem.onWarning, sem.warning, failures);
      check(
        'onWarningContainer/warningContainer',
        sem.onWarningContainer,
        sem.warningContainer,
        failures,
      );
      check('onInfo/info', sem.onInfo, sem.info, failures);
      check(
        'onInfoContainer/infoContainer',
        sem.onInfoContainer,
        sem.infoContainer,
        failures,
      );

      expect(failures, isEmpty, reason: failures.join('\n'));
    });
  }
}
