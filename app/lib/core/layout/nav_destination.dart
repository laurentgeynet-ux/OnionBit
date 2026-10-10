// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart' show AppLocalizations;

/// Identifiants des destinations top-level — les libellés affichés
/// sont localisés via [NavDestinationSpecX.label] (ARB).
enum NavId {
  downloads,
  search,
  channels,
  messages,
  diagnostic,
  settings,
  privateZone,
  about,
}

/// Description d'une destination de navigation top-level, partagée
/// entre la sidebar, la `NavigationBar` compacte et le routeur —
/// un seul catalogue, jamais dupliqué.
class NavDestinationSpec {
  const NavDestinationSpec({
    required this.path,
    required this.id,
    required this.icon,
    required this.selectedIcon,
    this.primary = true,
  });

  /// Chemin `go_router` (ex. `/downloads`).
  final String path;

  /// Identifiant de destination — libellé localisé via
  /// [NavDestinationSpecX.label].
  final NavId id;

  final IconData icon;
  final IconData selectedIcon;

  /// `true` pour les entrées visibles dans la nav compacte (barre du
  /// bas — max 5) ; `false` = secondaire, accessible par la sidebar.
  final bool primary;
}

extension NavDestinationSpecX on NavDestinationSpec {
  /// Libellé localisé de la destination.
  String label(AppLocalizations l10n) => switch (id) {
    NavId.downloads => l10n.navDownloads,
    NavId.search => l10n.navSearch,
    NavId.channels => l10n.navChannels,
    NavId.messages => l10n.navMessages,
    NavId.diagnostic => l10n.navDiagnostic,
    NavId.settings => l10n.navSettings,
    NavId.privateZone => l10n.navPrivateZone,
    NavId.about => l10n.navAbout,
  };
}
