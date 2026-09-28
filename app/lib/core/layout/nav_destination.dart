import 'package:flutter/material.dart';

/// Description d'une destination de navigation top-level, partagée
/// entre la sidebar, la `NavigationBar` compacte et le routeur —
/// un seul catalogue, jamais dupliqué.
class NavDestinationSpec {
  const NavDestinationSpec({
    required this.path,
    required this.label,
    required this.icon,
    required this.selectedIcon,
    this.primary = true,
  });

  /// Chemin `go_router` (ex. `/downloads`).
  final String path;

  /// Libellé affiché (français).
  final String label;

  final IconData icon;
  final IconData selectedIcon;

  /// `true` pour les entrées visibles dans la nav compacte (barre du
  /// bas — max 5) ; `false` = secondaire, accessible par la sidebar.
  final bool primary;
}
