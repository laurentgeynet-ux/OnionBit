import 'package:flutter/material.dart';

/// Design system Material 3 de l'application — repris à l'identique
/// de l'app de référence (`C:\Emule-Sion-UI-UX\app`).
///
/// Couleur de départ personnalisable (accent utilisateur) — la valeur
/// par défaut vit ici, une seule source de vérité.
abstract final class AppTheme {
  static const Color defaultSeedColor = Color(0xFF2F6FED);

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
    );
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
