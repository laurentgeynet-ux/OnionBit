// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

/// Design system Material 3 de l'application — repris de l'app de
/// référence (`C:\Emule-Sion-UI-UX\app`), recalé sur la palette de la
/// marque OnionBit (`branding/`).
///
/// Couleur de départ personnalisable (accent utilisateur) — la valeur
/// par défaut vit ici, une seule source de vérité.
abstract final class AppTheme {
  /// Violet ampoule du logo OnionBit (`#6C2EA6`).
  static const Color defaultSeedColor = Color(0xFF6C2EA6);

  /// Cyan flèche du logo OnionBit (`#4FD8E0`) — accent `tertiary`.
  static const Color brandTertiary = Color(0xFF4FD8E0);

  static ThemeData light({Color seedColor = defaultSeedColor}) =>
      _build(seedColor: seedColor, brightness: Brightness.light);

  static ThemeData dark({Color seedColor = defaultSeedColor}) =>
      _build(seedColor: seedColor, brightness: Brightness.dark);

  static ThemeData _build({
    required Color seedColor,
    required Brightness brightness,
  }) {
    final colorScheme = ColorScheme.fromSeed(
      seedColor: seedColor,
      brightness: brightness,
    ).copyWith(tertiary: brandTertiary);
    return ThemeData(
      useMaterial3: true,
      colorScheme: colorScheme,
      visualDensity: VisualDensity.standard,
      navigationRailTheme: NavigationRailThemeData(
        useIndicator: true,
        backgroundColor: colorScheme.surface,
      ),
      cardTheme: CardThemeData(
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppRadii.medium),
        ),
      ),
      // Le pouce par défaut de M3 (`onSurface` très dilué) est quasi
      // invisible en mode clair ; `outline` reste lisible dans les deux
      // modes, renforcé au survol/drag.
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

/// Tokens de design (espacements, rayons) — utilisés par tous les widgets
/// transverses (`core/widgets/`) pour éviter les magic numbers de layout.
abstract final class AppSpacing {
  static const double xs = 4;
  static const double sm = 8;
  static const double md = 16;
  static const double lg = 24;
  static const double xl = 32;
}

abstract final class AppRadii {
  static const double small = 8;
  static const double medium = 16;
  static const double large = 24;
}
