// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/widgets.dart';

/// Intents des raccourcis clavier transverses de la coquille
/// (ADR-0021 §6) — un type par action, mappé à une implémentation
/// dans `app_shortcuts.dart` (`Actions`). Séparer intent et action
/// permet de ré-assigner la touche sans toucher à la logique.
class GoToSearchIntent extends Intent {
  const GoToSearchIntent();
}

class AddDownloadIntent extends Intent {
  const AddDownloadIntent();
}

/// Ouvre la palette de commandes (Ctrl/Cmd+K, ADR-0021 §6).
class OpenCommandPaletteIntent extends Intent {
  const OpenCommandPaletteIntent();
}
