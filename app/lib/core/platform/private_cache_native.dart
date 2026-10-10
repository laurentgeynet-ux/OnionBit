// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Implémentation desktop (`dart:io`) du cache de lecture privé.
library;

import 'dart:io';

/// Dossier temporaire frais `onionbit_read_*` (sous la temp du
/// système — les copies en clair n'y survivent pas un nettoyage OS).
Future<String?> privateReadCacheDir() async {
  try {
    return (await Directory.systemTemp.createTemp('onionbit_read_')).path;
  } catch (_) {
    return null;
  }
}

String joinCachePath(String dir, String relpath) {
  final sep = Platform.pathSeparator;
  return '$dir$sep${relpath.split('/').join(sep)}';
}

const bool supportsPrivateReadCache = true;
