// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/animation.dart';

/// Durées d'animation (ADR-0021 §7) — trois paliers seulement,
/// délibérément peu nombreux pour rester cohérent partout.
abstract final class AppMotionDuration {
  /// Micro-interactions (survol, pression, bascule d'état d'un chip).
  static const Duration fast = Duration(milliseconds: 120);

  /// Transitions de contenu courantes (ouverture de panneau, fondu).
  static const Duration medium = Duration(milliseconds: 220);

  /// Transitions de page / changements de layout adaptatif majeurs.
  static const Duration slow = Duration(milliseconds: 400);
}

/// Courbes d'animation — `standard` reprend la courbe « emphasized »
/// Material 3 que Flutter utilise déjà en interne pour ses propres
/// transitions de page (`page_transitions_theme.dart`), plutôt que
/// d'inventer une variante maison.
abstract final class AppMotionCurve {
  static const Curve standard = Curves.easeInOutCubicEmphasized;
  static const Curve enter = Curves.easeOutCubic;
  static const Curve exit = Curves.easeInCubic;
}
