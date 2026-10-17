// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : sélecteur de fichiers natif `file_selector` —
/// l'UI tourne sur la même machine que le daemon.
library;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/widgets.dart';

Future<List<String>> pickDaemonFiles(
  BuildContext context, {
  String? initialPath,
}) async {
  final files = await openFiles(initialDirectory: initialPath);
  return [for (final f in files) f.path];
}
