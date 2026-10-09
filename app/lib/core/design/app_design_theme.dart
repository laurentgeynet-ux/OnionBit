// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/cupertino.dart' show CupertinoPageTransitionsBuilder;
import 'package:flutter/material.dart';

import 'tokens/color_tokens.dart';
import 'tokens/radius_tokens.dart';
import 'tokens/typography_tokens.dart';

/// Design system OnionBit (ADR-0021) — successeur de l'ancien
/// `core/theme/app_theme.dart` (`AppTheme`), qui l'a remplacé après
/// la migration écran par écran (§2/§10 : étape 77 = suppression de
/// l'ancien, zéro référence restante). La couleur de marque ne change
/// pas (`AppBrandColors.seed`) : seul le système qui la porte a été
/// reconstruit.
abstract final class AppDesignTheme {
  static ThemeData light({Color seedColor = AppBrandColors.seed}) =>
      _build(seedColor: seedColor, brightness: Brightness.light);

  static ThemeData dark({Color seedColor = AppBrandColors.seed}) =>
      _build(seedColor: seedColor, brightness: Brightness.dark);

  static ThemeData _build({
    required Color seedColor,
    required Brightness brightness,
  }) {
    final colorScheme = ColorScheme.fromSeed(
      seedColor: seedColor,
      brightness: brightness,
    ).copyWith(
      tertiary: AppBrandColors.tertiary,
      // Le cyan de marque est trop clair pour du texte blanc (1.7:1,
      // loin du AA — vérifié par test/design/contrast_test.dart) :
      // `onTertiary` foncé dans les deux modes.
      onTertiary: const Color(0xFF00252B),
    );
    final semantic = brightness == Brightness.light
        ? AppSemanticColors.light
        : AppSemanticColors.dark;

    return ThemeData(
      useMaterial3: true,
      colorScheme: colorScheme,
      textTheme: AppTypography.textTheme(
        ThemeData(brightness: brightness, useMaterial3: true).textTheme,
      ),
      visualDensity: VisualDensity.standard,
      extensions: [AppSemanticColorsExtension(semantic)],
      // Apple-ready dès le premier jour (ADR-0021 §3/Limites) : la
      // transition « glissement » native sur iOS/macOS, celle
      // d'Android ailleurs — aucune des deux cibles ne nécessitera de
      // retouche quand leurs runners seront scaffoldés (§4).
      pageTransitionsTheme: const PageTransitionsTheme(
        builders: {
          TargetPlatform.android: ZoomPageTransitionsBuilder(),
          TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
          TargetPlatform.macOS: CupertinoPageTransitionsBuilder(),
          TargetPlatform.linux: ZoomPageTransitionsBuilder(),
          TargetPlatform.windows: ZoomPageTransitionsBuilder(),
          TargetPlatform.fuchsia: ZoomPageTransitionsBuilder(),
        },
      ),
      navigationRailTheme: NavigationRailThemeData(
        useIndicator: true,
        backgroundColor: colorScheme.surface,
      ),
      cardTheme: CardThemeData(
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppRadius.medium),
        ),
      ),
      // Le pouce par défaut de M3 (`onSurface` très dilué) est quasi
      // invisible en mode clair ; `outline` reste lisible dans les
      // deux modes, renforcé au survol/drag (repris de l'ancien
      // `AppTheme`, ce réglage n'est pas de la dette).
      scrollbarTheme: ScrollbarThemeData(
        thumbColor: WidgetStateProperty.resolveWith(
          (states) =>
              states.contains(WidgetState.hovered) ||
                  states.contains(WidgetState.dragged)
              ? colorScheme.onSurfaceVariant
              : colorScheme.outline,
        ),
      ),
    );
  }
}
