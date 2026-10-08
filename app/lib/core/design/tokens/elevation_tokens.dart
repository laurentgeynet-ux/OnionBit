// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Paliers d'élévation Material 3 (dp), nommés pour éviter les nombres
/// magiques dans `core/widgets/`/`features/*/presentation/` — valeurs
/// alignées sur l'échelle M3 standard (0/1/3/6/8/12).
///
/// Pour les moments de marque (panneaux contextuels, Privacy HUD),
/// préférer `FrostedSurface` (dégradé + flou, ADR-0021 §7) à une
/// élévation Material plate.
abstract final class AppElevation {
  static const double level0 = 0;
  static const double level1 = 1;
  static const double level2 = 3;
  static const double level3 = 6;
  static const double level4 = 8;
  static const double level5 = 12;
}
