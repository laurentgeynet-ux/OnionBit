import 'package:flutter/material.dart';

import '../layout/nav_destination.dart';

/// Catalogue unique des destinations top-level — une ligne par feature.
/// `Téléchargements` porte en plus les sous-filtres affichés dans la
/// sidebar (cf. `app_sidebar.dart`, `DownloadFilter`).
const List<NavDestinationSpec> kNavCatalog = [
  NavDestinationSpec(
    path: '/downloads',
    label: 'Téléchargements',
    icon: Icons.download_outlined,
    selectedIcon: Icons.download,
  ),
  NavDestinationSpec(
    path: '/search',
    label: 'Rechercher',
    icon: Icons.search_outlined,
    selectedIcon: Icons.search,
  ),
  NavDestinationSpec(
    path: '/diagnostic',
    label: 'Diagnostic',
    icon: Icons.monitor_heart_outlined,
    selectedIcon: Icons.monitor_heart,
  ),
  NavDestinationSpec(
    path: '/settings',
    label: 'Réglages',
    icon: Icons.settings_outlined,
    selectedIcon: Icons.settings,
  ),
];
