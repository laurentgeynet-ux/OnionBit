// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : sélecteur de dossier natif `file_selector` —
/// l'UI tourne sur la même machine que le daemon.
library;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/widgets.dart';

Future<String?> pickDaemonDirectory(
  BuildContext context, {
  String? initialPath,
}) => getDirectoryPath(initialDirectory: initialPath);
