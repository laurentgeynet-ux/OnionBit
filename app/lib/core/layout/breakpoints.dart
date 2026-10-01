// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Points de rupture responsive de l'application.
///
/// Source unique de vérité — tout widget qui a besoin de savoir
/// « suis-je en mode compact ? » passe par [AppBreakpoints.of].
enum AppBreakpoint {
  /// < 600 dp — mobile / fenêtre très étroite (nav basse, une colonne).
  compact,

  /// 600–1024 dp — tablette portrait / petite fenêtre desktop.
  medium,

  /// 1024–1600 dp — tablette paysage / desktop.
  expanded,

  /// ≥ 1600 dp — desktop grand écran (multi-panneaux).
  large,
}

abstract final class AppBreakpoints {
  static const double medium = 600;
  static const double expanded = 1024;
  static const double large = 1600;

  static AppBreakpoint of(double width) {
    if (width >= large) return AppBreakpoint.large;
    if (width >= expanded) return AppBreakpoint.expanded;
    if (width >= medium) return AppBreakpoint.medium;
    return AppBreakpoint.compact;
  }
}
