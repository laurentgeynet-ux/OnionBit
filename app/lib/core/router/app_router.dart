// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/about/presentation/pages/about_page.dart';
import '../../features/diagnostic/presentation/pages/diagnostic_page.dart';
import '../../features/downloads/domain/download_filter.dart';
import '../../features/downloads/presentation/pages/downloads_page.dart';
import '../../features/search/presentation/pages/search_page.dart';
import '../../features/settings/presentation/pages/settings_page.dart';
import '../layout/app_shell.dart';

/// Routeur applicatif unique (`go_router`) — un `ShellRoute` porte le
/// chrome commun (`AppShell` : sidebar/navbar + top bar + status bar),
/// chaque destination du catalogue a une route dédiée vers sa feature.
final appRouterProvider = Provider<GoRouter>((ref) {
  return GoRouter(
    initialLocation: '/downloads',
    routes: [
      ShellRoute(
        builder: (context, state, child) => AppShell(
          currentPath: state.matchedLocation,
          currentQuery: state.uri.queryParameters,
          child: child,
        ),
        routes: [
          GoRoute(
            path: '/downloads',
            builder: (context, state) => DownloadsPage(
              filter: DownloadFilter.fromQuery(state.uri.queryParameters['f']),
            ),
          ),
          GoRoute(
            path: '/search',
            builder: (context, state) => const SearchPage(),
          ),
          GoRoute(
            path: '/diagnostic',
            builder: (context, state) => const DiagnosticPage(),
          ),
          GoRoute(
            path: '/settings',
            builder: (context, state) => const SettingsPage(),
          ),
          GoRoute(
            path: '/about',
            builder: (context, state) => const AboutPage(),
          ),
        ],
      ),
    ],
  );
});
