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
    path: '/channels',
    id: NavId.channels,
    icon: Icons.collections_bookmark_outlined,
    selectedIcon: Icons.collections_bookmark,
    // Secondaire comme `/about` : la nav compacte est bornee a 5
    // (downloads, search, messages, diagnostic, settings).
    primary: false,
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
  // Secondaires : sidebar uniquement (hors barre de nav compacte).
  // `/private-zone` (ADR-0027) : la sidebar ne le montre que si la
  // zone est montee (cf. `app_sidebar.dart`).
  NavDestinationSpec(
    path: '/private-zone',
    id: NavId.privateZone,
    icon: Icons.enhanced_encryption_outlined,
    selectedIcon: Icons.enhanced_encryption,
    primary: false,
  ),
  NavDestinationSpec(
    path: '/about',
    id: NavId.about,
    icon: Icons.info_outline,
    selectedIcon: Icons.info,
    primary: false,
  ),
];
