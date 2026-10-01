// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'download.dart';

/// Filtres de la liste des téléchargements (sous-menu sidebar, clé de
/// query `?f=` dans l'URL — deep-linkable).
enum DownloadFilter {
  /// Tous les téléchargements.
  all('Tous', ''),

  /// En cours de téléchargement (`DOWNLOADING`, métadonnées, check).
  downloading('En cours', 'downloading'),

  /// Terminés (progression 100 % ou `SEEDING`).
  completed('Terminés', 'completed'),

  /// Actifs : du trafic en cours (up ou down).
  active('Actifs', 'active'),

  /// Inactifs : arrêtés/erreur ou sans trafic.
  inactive('Inactifs', 'inactive');

  const DownloadFilter(this.label, this.queryKey);

  final String label;

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
