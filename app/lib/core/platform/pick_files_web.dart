// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : dialogue naviguant `/api/files/browse` en mode fichiers —
/// un navigateur n'a pas de sélecteur de fichiers pour la machine
/// du daemon.
library;

import 'package:flutter/widgets.dart';

import '../widgets/daemon_directory_picker.dart';

Future<List<String>> pickDaemonFiles(
  BuildContext context, {
  String? initialPath,
}) async {
  final path = await DaemonDirectoryPicker.showFile(
    context,
    initialPath: initialPath,
  );
  return path == null ? const [] : [path];
}
