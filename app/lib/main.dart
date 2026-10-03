// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'app.dart';
import 'core/di/providers.dart';
import 'core/platform/desktop_shell.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  if (kDebugMode) {
    debugPrint('Démarrage (${kIsWeb ? 'web' : defaultTargetPlatform.name})');
  }
  await initDesktopShell();
  // Web : supprime le menu contextuel natif du navigateur, qui sinon
  // s'ouvre par-dessus les menus Flutter (`MenuAnchor`) au clic droit.
  // Les champs de texte gardent leur menu Flutter (copier/coller).
  if (kIsWeb) {
    await BrowserContextMenu.disableContextMenu();
  }
  // « Ouvrir avec » / association .torrent : le chemin arrive en argv.
  final startupFiles = args
      .map((a) => a.trim())
      .where((a) => a.toLowerCase().endsWith('.torrent') ||
          a.toLowerCase().endsWith('.magnet'))
      .toList();
  runApp(
    ProviderScope(
      overrides: [
        startupFilesProvider.overrideWithValue(startupFiles),
      ],
      child: const OnionbitApp(),
    ),
  );
}
