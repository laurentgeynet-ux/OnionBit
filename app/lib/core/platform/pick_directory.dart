// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Choix d'un dossier **côté daemon**, spécifique plateforme.
///
/// Sur desktop (`dart.library.io`) : sélecteur natif `file_selector`.
/// Sur web : dialogue naviguant `/api/files/browse` — les chemins
/// attendus par l'API (destination, watch folder, `move_storage`)
/// appartiennent à la machine du daemon, pas au navigateur.
library;

import 'package:flutter/widgets.dart';

import 'pick_directory_web.dart'
    if (dart.library.io) 'pick_directory_native.dart' as impl;

/// Renvoie le chemin choisi (`null` = annulé).
Future<String?> pickDaemonDirectory(
  BuildContext context, {
  String? initialPath,
}) => impl.pickDaemonDirectory(context, initialPath: initialPath);
