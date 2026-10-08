// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../l10n/app_localizations.dart' show AppLocalizations;
import 'download.dart';

/// Filtres de la liste des téléchargements (sous-menu sidebar, clé de
/// query `?f=` dans l'URL — deep-linkable). Les libellés affichés
/// sont localisés via [DownloadFilterX.label] (ARB).
enum DownloadFilter {
  /// Tous les téléchargements.
  all(''),

  /// En cours de téléchargement (`DOWNLOADING`, métadonnées, check).
  downloading('downloading'),

  /// Terminés (progression 100 % ou `SEEDING`).
  completed('completed'),

  /// Actifs : du trafic en cours (up ou down).
  active('active'),

  /// Inactifs : arrêtés/erreur ou sans trafic.
  inactive('inactive');

  const DownloadFilter(this.queryKey);

  /// Valeur du paramètre `?f=` (`''` = absence de paramètre).
  final String queryKey;

  static DownloadFilter fromQuery(String? value) => DownloadFilter.values
      .firstWhere((f) => f.queryKey == (value ?? ''), orElse: () => all);

  bool matches(Download d) => switch (this) {
    all => true,
    downloading => const {
      'DOWNLOADING',
      'METADATA',
      'HASHCHECKING',
      'WAITING_FOR_HASHCHECK',
    }.contains(d.status),
    completed => d.progress >= 1.0 || d.status == 'SEEDING',
    active => d.speedDown > 0 || d.speedUp > 0,
    inactive =>
      d.status == 'STOPPED' ||
          d.status == 'STOPPED_ON_ERROR' ||
          (d.speedDown == 0 && d.speedUp == 0),
  };
}

extension DownloadFilterX on DownloadFilter {
  /// Libellé localisé du filtre.
  String label(AppLocalizations l10n) => switch (this) {
    DownloadFilter.all => l10n.filterAll,
    DownloadFilter.downloading => l10n.filterDownloading,
    DownloadFilter.completed => l10n.filterCompleted,
    DownloadFilter.active => l10n.filterActive,
    DownloadFilter.inactive => l10n.filterInactive,
  };
}
