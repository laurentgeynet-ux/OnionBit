// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../l10n/l10n_ext.dart';
import '../router/nav_catalog.dart';
import 'app_sidebar.dart';
import 'breakpoints.dart';
import 'daemon_unreachable_banner.dart';
import 'drop_zone.dart';
import '../di/providers.dart';
import '../notifications/notifications_listener.dart';
import 'pending_files_handler.dart';
import 'status_bar.dart';
import 'top_bar.dart';

/// Coquille responsive de l'application.
///
/// - `compact` (< 600 dp) : `NavigationBar` en bas (destinations
///   primaires), le détail des téléchargements passe en bottom sheet.
/// - ≥ 600 dp : sidebar fixe type Tribler à gauche.
/// Les deux variantes portent la barre de recherche en haut et la
/// barre d'état en bas.
class AppShell extends ConsumerWidget {
  const AppShell({
    super.key,
    required this.currentPath,
    required this.currentQuery,
    required this.child,
    this.banner,
  });

  final String currentPath;
  final Map<String, String> currentQuery;
  final Widget child;

  /// Bannière transverse (ex. daemon injoignable) au-dessus du contenu.
  final Widget? banner;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final breakpoint = AppBreakpoints.of(MediaQuery.sizeOf(context).width);
    final sse = ref.watch(sseConnectedProvider);
    final unreachable = sse.value == false && !sse.isLoading;

    final body = DropZone(
      child: Column(
        children: [
          const TopBar(),
          ?banner,
          if (unreachable) const DaemonUnreachableBanner(),
          Expanded(child: child),
          const StatusBar(),
        ],
      ),
    );

    if (breakpoint == AppBreakpoint.compact) {
      final destinations = kNavCatalog.where((d) => d.primary).toList();
      final selected = destinations
          .indexWhere((d) => currentPath.startsWith(d.path))
          .clamp(0, destinations.length - 1);
      return Scaffold(
        body: SafeArea(
          child: Stack(
            children: [
              Positioned.fill(child: body),
              const Positioned.fill(child: PendingFilesHandler()),
              const Positioned.fill(child: NotificationsListener()),
            ],
          ),
        ),
        bottomNavigationBar: NavigationBar(
          selectedIndex: selected,
          onDestinationSelected: (i) => context.go(destinations[i].path),
          destinations: [
            for (final d in destinations)
              NavigationDestination(
                icon: Icon(d.icon),
                selectedIcon: Icon(d.selectedIcon),
                label: d.label(context.l10n),
              ),
          ],
        ),
      );
    }

    return Scaffold(
      body: Row(
        children: [
          AppSidebar(currentPath: currentPath, currentQuery: currentQuery),
          const VerticalDivider(width: 1),
          Expanded(
            child: SafeArea(
              child: Stack(
                children: [
                  Positioned.fill(child: body),
                  const Positioned.fill(child: PendingFilesHandler()),
                  const Positioned.fill(child: NotificationsListener()),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
