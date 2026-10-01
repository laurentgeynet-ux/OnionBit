import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'core/config/ui_prefs.dart';
import 'core/di/providers.dart';
import 'core/router/app_router.dart';
import 'core/theme/app_theme.dart';
import 'core/theme/theme_settings.dart';

/// Racine de l'application — thème Material 3 + routeur `go_router`.
class OnionbitApp extends ConsumerWidget {
  const OnionbitApp({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // Démarre le flux SSE une seule fois, à la racine.
    ref.watch(sseClientProvider);
    // Seme les préférences UI persistées (rail, filtres, tris) dans
    // les notifiers de session — écriture write-through ensuite.
    ref.watch(uiPrefsInitProvider);
    final router = ref.watch(appRouterProvider);
    // Accent + mode persistés ; repli sur les défauts tant que les
    // préférences ne sont pas chargées.
    final appearance = ref.watch(themeSettingsProvider).value;
    final seed = appearance?.seedColor ?? AppTheme.defaultSeedColor;
    final mode = appearance?.mode ?? ThemeMode.system;

    return MaterialApp.router(
      title: 'OnionBit',
      debugShowCheckedModeBanner: false,
      theme: AppTheme.light(seedColor: seed),
      darkTheme: AppTheme.dark(seedColor: seed),
      themeMode: mode,
      routerConfig: router,
    );
  }
}
