// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Choix de fichiers **côté daemon**, spécifique plateforme —
/// pendant de `pick_directory.dart` pour les imports (zone
/// publique, zone privée).
///
/// Sur desktop (`dart.library.io`) : sélecteur natif `file_selector`.
/// Sur web : dialogue naviguant `/api/files/browse` en mode
/// fichiers — un navigateur n'énumère pas le système de fichiers
/// du daemon (qui peut être une autre machine).
library;

import 'package:flutter/widgets.dart';

import 'pick_files_web.dart'
    if (dart.library.io) 'pick_files_native.dart' as impl;

/// Renvoie les chemins fichiers choisis (liste vide = annulé).
Future<List<String>> pickDaemonFiles(
  BuildContext context, {
  String? initialPath,
}) => impl.pickDaemonFiles(context, initialPath: initialPath);
