import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
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
      child: const TriblerApp(),
    ),
  );
}
