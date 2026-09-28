import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'app.dart';
import 'core/platform/desktop_shell.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (kDebugMode) {
    debugPrint('Démarrage (${kIsWeb ? 'web' : defaultTargetPlatform.name})');
  }
  await initDesktopShell();
  runApp(const ProviderScope(child: TriblerApp()));
}
