// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Échelle de rayons (ADR-0021 §7) — succède à `AppRadii` de l'ancien
/// `core/theme/app_theme.dart` (mêmes valeurs ; voir `AppSpace`).
abstract final class AppRadius {
  static const double none = 0;
  static const double small = 8;
  static const double medium = 16;
  static const double large = 24;

  /// Forme « pilule » (chips, boutons ronds) — rayon volontairement
  /// supérieur à la moitié de la plus grande dimension attendue.
  static const double full = 999;
}
