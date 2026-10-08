// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../layout/nav_destination.dart';

export '../layout/nav_destination.dart';

/// Catalogue unique des destinations top-level — une ligne par feature.
/// `NavId.downloads` porte en plus les sous-filtres affichés dans la
/// sidebar (cf. `app_sidebar.dart`, `DownloadFilter`).
const List<NavDestinationSpec> kNavCatalog = [
  NavDestinationSpec(
    path: '/downloads',
    id: NavId.downloads,
    icon: Icons.download_outlined,
    selectedIcon: Icons.download,
  ),
  NavDestinationSpec(
    path: '/search',
    id: NavId.search,
    icon: Icons.search_outlined,
    selectedIcon: Icons.search,
  ),
  NavDestinationSpec(
    path: '/messages',
    id: NavId.messages,
    icon: Icons.forum_outlined,
    selectedIcon: Icons.forum,
  ),
  NavDestinationSpec(
    path: '/diagnostic',
    id: NavId.diagnostic,
    icon: Icons.monitor_heart_outlined,
    selectedIcon: Icons.monitor_heart,
  ),
  NavDestinationSpec(
    path: '/settings',
    id: NavId.settings,
    icon: Icons.settings_outlined,
    selectedIcon: Icons.settings,
  ),
  // Secondaire : sidebar uniquement (hors barre de nav compacte).
  NavDestinationSpec(
    path: '/about',
    id: NavId.about,
    icon: Icons.info_outline,
    selectedIcon: Icons.info,
    primary: false,
  ),
];
