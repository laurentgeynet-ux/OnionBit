import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../router/nav_catalog.dart';
import 'app_sidebar.dart';
import 'breakpoints.dart';
import 'drop_zone.dart';
import '../notifications/notifications_listener.dart';
import 'pending_files_handler.dart';
import 'status_bar.dart';
import 'top_bar.dart';
import 'torrent_finished_listener.dart';

/// Coquille responsive de l'application.
///
/// - `compact` (< 600 dp) : `NavigationBar` en bas (destinations
///   primaires), le détail des téléchargements passe en bottom sheet.
/// - ≥ 600 dp : sidebar fixe type Tribler à gauche.
/// Les deux variantes portent la barre de recherche en haut et la
/// barre d'état en bas.
class AppShell extends StatelessWidget {
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
  Widget build(BuildContext context) {
    final breakpoint = AppBreakpoints.of(MediaQuery.sizeOf(context).width);

    final body = DropZone(
      child: Column(
        children: [
          const TopBar(),
          ?banner,
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
              const Positioned.fill(child: TorrentFinishedListener()),
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
                label: d.label,
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
                  const Positioned.fill(child: TorrentFinishedListener()),
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
