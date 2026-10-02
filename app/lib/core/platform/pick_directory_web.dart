// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : dialogue naviguant `/api/files/browse` — un navigateur n'a
/// pas de sélecteur de dossier natif, et le chemin attendu est celui
/// du daemon (qui peut être une autre machine).
library;

import 'package:flutter/widgets.dart';

import '../widgets/daemon_directory_picker.dart';

Future<String?> pickDaemonDirectory(
  BuildContext context, {
  String? initialPath,
}) => DaemonDirectoryPicker.show(context, initialPath: initialPath);
