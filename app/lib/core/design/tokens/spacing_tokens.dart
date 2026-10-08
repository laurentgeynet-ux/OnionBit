// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Échelle d'espacement (ADR-0021 §7) — succède à `AppSpacing` de
/// l'ancien `core/theme/app_theme.dart` (mêmes valeurs : la migration
/// des écrans est un renommage, jamais un changement visuel). Les deux
/// coexistent le temps de la migration (ADR-0021 §2).
abstract final class AppSpace {
  static const double none = 0;
  static const double xs = 4;
  static const double sm = 8;
  static const double md = 16;
  static const double lg = 24;
  static const double xl = 32;
  static const double xxl = 48;
}
