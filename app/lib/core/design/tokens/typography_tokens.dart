// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

/// Noms de famille des trois polices auto-hébergées (ADR-0021 §3,
/// provenance : `assets/fonts/SOURCES.md`). Polices variables : une
/// seule fonte par famille, plusieurs graisses déclarées dans
/// `pubspec.yaml` (`fonts:`) pour rester compatibles avec l'API
/// standard `TextStyle(fontWeight:)`.
abstract final class AppFontFamilies {
  /// Titres, identité visuelle — jamais le texte courant.
  static const String display = 'Space Grotesk';

  /// Texte d'interface par défaut (corps, libellés, boutons).
  static const String body = 'Inter';

  /// Hex, infohashes, clés, phrases de seed (ADR-0016) — jamais la
  /// police de corps : JetBrains Mono distingue sans ambiguïté `0`/`O`
  /// et `1`/`l`, critique quand une confusion sur une seed phrase est
  /// irréversible.
  static const String mono = 'JetBrains Mono';
}

/// Échelle typographique (ADR-0021 §7) : Space Grotesk pour les rôles
/// « display »/« headline »/« titleLarge » (identité de marque), Inter
/// pour le reste — construite par-dessus l'échelle Material 3 par
/// défaut plutôt que réinventée.
abstract final class AppTypography {
  static TextTheme textTheme(TextTheme base) => base.copyWith(
    displayLarge: base.displayLarge?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    displayMedium: base.displayMedium?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    displaySmall: base.displaySmall?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    headlineLarge: base.headlineLarge?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    headlineMedium: base.headlineMedium?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    headlineSmall: base.headlineSmall?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w600,
    ),
    titleLarge: base.titleLarge?.copyWith(
      fontFamily: AppFontFamilies.display,
      fontWeight: FontWeight.w500,
    ),
    titleMedium: base.titleMedium?.copyWith(
      fontFamily: AppFontFamilies.body,
      fontWeight: FontWeight.w600,
    ),
    titleSmall: base.titleSmall?.copyWith(
      fontFamily: AppFontFamilies.body,
      fontWeight: FontWeight.w600,
    ),
    bodyLarge: base.bodyLarge?.copyWith(fontFamily: AppFontFamilies.body),
    bodyMedium: base.bodyMedium?.copyWith(fontFamily: AppFontFamilies.body),
    bodySmall: base.bodySmall?.copyWith(fontFamily: AppFontFamilies.body),
    labelLarge: base.labelLarge?.copyWith(
      fontFamily: AppFontFamilies.body,
      fontWeight: FontWeight.w600,
    ),
    labelMedium: base.labelMedium?.copyWith(
      fontFamily: AppFontFamilies.body,
      fontWeight: FontWeight.w600,
    ),
    labelSmall: base.labelSmall?.copyWith(
      fontFamily: AppFontFamilies.body,
      fontWeight: FontWeight.w600,
    ),
  );

  /// Style monospace explicite — à piocher partout où un hex/une
  /// clé/une phrase de seed s'affiche (jamais via `textTheme`, qui ne
  /// porte aucun rôle monospace).
  static TextStyle mono({
    double fontSize = 14,
    FontWeight fontWeight = FontWeight.w400,
    double? letterSpacing,
  }) => TextStyle(
    fontFamily: AppFontFamilies.mono,
    fontSize: fontSize,
    fontWeight: fontWeight,
    letterSpacing: letterSpacing,
  );
}
