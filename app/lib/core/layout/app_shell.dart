// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../l10n/l10n_ext.dart';
import '../router/nav_catalog.dart';
import '../shortcuts/app_shortcuts.dart';
import 'app_sidebar.dart';
import 'breakpoints.dart';
import 'daemon_unreachable_banner.dart';
import 'drop_zone.dart';
import '../di/providers.dart';
import '../notifications/notifications_listener.dart';
import 'pending_files_handler.dart';
import 'status_bar.dart';
import 'top_bar.dart';

/// Coquille responsive de l'application (ADR-0021 §5) :
///
/// - `compact` (< 600 dp) : `NavigationBar` en bas, une colonne, le
///   détail des téléchargements passe en bottom sheet.
/// - `medium` (600-1024 dp) : sidebar forcée en rail (icônes seules) —
///   pas assez de largeur pour justifier un choix utilisateur.
/// - `expanded`/`large` (≥ 1024 dp) : sidebar pleine largeur par
///   défaut, repliable manuellement (préférence persistée,
///   `sidebarCollapsedProvider`) — comportement historique inchangé.
///
/// Toutes les variantes portent la barre de recherche en haut, la
/// barre d'état en bas, les raccourcis clavier transverses
/// (`AppShortcuts`) et un ordre de focus explicite par panneau
/// (`FocusTraversalGroup`).
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
      return AppShortcuts(
        child: Scaffold(
          body: SafeArea(
            child: Stack(
              children: [
                Positioned.fill(child: FocusTraversalGroup(child: body)),
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
        ),
      );
    }

    // `medium` : rail forcé (pas de choix utilisateur, largeur trop
    // juste) ; `expanded`/`large` : préférence persistée (historique,
    // `collapsedOverride: null` laisse `AppSidebar` lire le provider).
    final sidebar = AppSidebar(
      currentPath: currentPath,
      currentQuery: currentQuery,
      collapsedOverride: breakpoint == AppBreakpoint.medium ? true : null,
    );

    return AppShortcuts(
      child: Scaffold(
        body: Row(
          children: [
            FocusTraversalGroup(child: sidebar),
            const VerticalDivider(width: 1),
            Expanded(
              child: SafeArea(
                child: Stack(
                  children: [
                    Positioned.fill(child: FocusTraversalGroup(child: body)),
                    const Positioned.fill(child: PendingFilesHandler()),
                    const Positioned.fill(child: NotificationsListener()),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
