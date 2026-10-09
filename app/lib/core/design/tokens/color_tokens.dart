// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

/// Couleurs de marque OnionBit — reprises telles quelles de l'ancien
/// `core/theme/app_theme.dart` (ADR-0021 : la marque ne change pas,
/// seul le système qui l'exploite est reconstruit, §1).
abstract final class AppBrandColors {
  /// Violet ampoule du logo — couleur de départ (seed) du schéma
  /// Material 3.
  static const Color seed = Color(0xFF6C2EA6);

  /// Cyan flèche du logo — accent `tertiary`.
  static const Color tertiary = Color(0xFF4FD8E0);
}

/// Tokens de couleur sémantiques (ADR-0021 §7) qui complètent
/// `ColorScheme` — Material 3 fournit déjà `error`, mais aucun
/// équivalent standard pour succès/avertissement/info.
///
/// Les paliers de surface/élévation n'ont volontairement pas de token
/// dédié ici : `ColorScheme.surfaceContainerLowest/Low/.../Highest`
/// (Material 3) les couvre nativement — les réinventer serait une
/// divergence inutile du système que Flutter maintient déjà.
class AppSemanticColors {
  const AppSemanticColors({
    required this.success,
    required this.onSuccess,
    required this.successContainer,
    required this.onSuccessContainer,
    required this.warning,
    required this.onWarning,
    required this.warningContainer,
    required this.onWarningContainer,
    required this.info,
    required this.onInfo,
    required this.infoContainer,
    required this.onInfoContainer,
  });

  final Color success;
  final Color onSuccess;
  final Color successContainer;
  final Color onSuccessContainer;
  final Color warning;
  final Color onWarning;
  final Color warningContainer;
  final Color onWarningContainer;
  final Color info;
  final Color onInfo;
  final Color infoContainer;
  final Color onInfoContainer;

  static const light = AppSemanticColors(
    success: Color(0xFF2E7D4F),
    onSuccess: Color(0xFFFFFFFF),
    successContainer: Color(0xFFD4F3DF),
    onSuccessContainer: Color(0xFF0B3D20),
    warning: Color(0xFF8A5B00),
    onWarning: Color(0xFFFFFFFF),
    warningContainer: Color(0xFFFFE3AC),
    onWarningContainer: Color(0xFF2B1B00),
    info: Color(0xFF2F6FED),
    onInfo: Color(0xFFFFFFFF),
    infoContainer: Color(0xFFD7E3FF),
    onInfoContainer: Color(0xFF0B2559),
  );

  static const dark = AppSemanticColors(
    success: Color(0xFF8FDBA8),
    onSuccess: Color(0xFF00391A),
    successContainer: Color(0xFF14512D),
    onSuccessContainer: Color(0xFFD4F3DF),
    warning: Color(0xFFFFC14D),
    onWarning: Color(0xFF462D00),
    warningContainer: Color(0xFF654300),
    onWarningContainer: Color(0xFFFFE3AC),
    info: Color(0xFFABC6FF),
    onInfo: Color(0xFF002C71),
    infoContainer: Color(0xFF1848A0),
    onInfoContainer: Color(0xFFD7E3FF),
  );
}

/// `ThemeExtension` portant [AppSemanticColors] — enregistrée par
/// `AppDesignTheme` (`extensions:`), lue via `context.semanticColors`.
class AppSemanticColorsExtension
    extends ThemeExtension<AppSemanticColorsExtension> {
  const AppSemanticColorsExtension(this.colors);

  final AppSemanticColors colors;

  @override
  AppSemanticColorsExtension copyWith({AppSemanticColors? colors}) =>
      AppSemanticColorsExtension(colors ?? this.colors);

  @override
  AppSemanticColorsExtension lerp(
    ThemeExtension<AppSemanticColorsExtension>? other,
    double t,
  ) {
    if (other is! AppSemanticColorsExtension) return this;
    // Pas d'interpolation visuelle entre statuts — bascule nette.
    return t < 0.5 ? this : other;
  }
}

/// Raccourci d'accès aux tokens sémantiques depuis un `BuildContext`.
/// Replie sur la palette claire si l'extension n'est pas enregistrée
/// (ex. widget testé hors `AppDesignTheme`).
extension AppSemanticColorsContext on BuildContext {
  AppSemanticColors get semanticColors =>
      Theme.of(this).extension<AppSemanticColorsExtension>()?.colors ??
      AppSemanticColors.light;
}
