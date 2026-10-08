// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : détecteur `desktop_drop` — les fichiers
/// arrivent par chemin (`path`), lus différément par le dialogue.
library;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../platform/pick_file.dart';

/// Détecteur de glisser-déposer natif (même API que la variante web).
class DropDetector extends StatelessWidget {
  const DropDetector({
    super.key,
    required this.onHover,
    required this.onFiles,
    required this.child,
  });

  /// Survol d'un dépôt (liseré visuel).
  final void Function(bool hovering) onHover;

  /// Fichiers lâchés (avant filtrage `.torrent`/`.magnet`).
  final void Function(List<PickedFile> files) onFiles;

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return DropTarget(
      onDragEntered: (_) => onHover(true),
      onDragExited: (_) => onHover(false),
      onDragDone: (details) {
        onHover(false);
        onFiles([
          for (final f in details.files)
            PickedFile(name: f.name, path: f.path),
        ]);
      },
      child: child,
    );
  }
}
