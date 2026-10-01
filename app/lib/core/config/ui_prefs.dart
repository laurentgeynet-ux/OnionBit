// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../features/downloads/presentation/providers/downloads_providers.dart';
import '../../features/search/presentation/providers/search_providers.dart';
import '../layout/app_sidebar.dart';

/// Préférences d'interface persistées (`shared_preferences`) —
/// initialisées une fois au démarrage : ce provider lit les clés et
/// sème les notifiers de session correspondants. Les notifiers
/// écrivent eux-mêmes leur clé à chaque mutation (write-through) ;
/// aucune resynchronisation n'est nécessaire ensuite.
///
/// Clés : `ui.sidebarCollapsed`, `ui.filtersExpanded`,
/// `ui.downloadSort` (`"<col>:<asc>"`), `ui.searchColSort` (idem,
/// chaîne vide = tri pertinence).
final uiPrefsInitProvider = FutureProvider<void>((ref) async {
  final prefs = await SharedPreferences.getInstance();

  ref
      .read(sidebarCollapsedProvider.notifier)
      .init(prefs.getBool('ui.sidebarCollapsed') ?? false);
  ref
      .read(sidebarFiltersExpandedProvider.notifier)
      .init(prefs.getBool('ui.filtersExpanded') ?? true);

  final ds = prefs.getString('ui.downloadSort');
  if (ds != null) {
    final parts = ds.split(':');
    final col = DownloadSort.values.asNameMap()[parts.first];
    if (col != null) {
      ref.read(downloadSortProvider.notifier).init((
        col: col,
        asc: parts.last == 'asc',
      ));
    }
  }

  final vm = prefs.getString('ui.downloadViewMode');
  final mode = DownloadViewMode.values.asNameMap()[vm];
  if (mode != null) {
    ref.read(downloadViewModeProvider.notifier).init(mode);
  }

  final dh = prefs.getDouble('ui.detailPanelHeight');
  if (dh != null) {
    ref.read(detailPanelHeightProvider.notifier).init(dh);
  }

  final ss = prefs.getString('ui.searchColSort');
  if (ss != null) {
    if (ss.isEmpty) {
      ref.read(searchColSortProvider.notifier).init(null);
    } else {
      final parts = ss.split(':');
      final col = SearchCol.values.asNameMap()[parts.first];
      if (col != null) {
        ref.read(searchColSortProvider.notifier).init((
          col: col,
          asc: parts.last == 'asc',
        ));
      }
    }
  }
});

/// Écriture write-through — appelée par les notifiers de préférence
/// après chaque mutation.
Future<void> uiPrefsWrite(String key, Object? value) async {
  final prefs = await SharedPreferences.getInstance();
  if (value == null) {
    await prefs.remove(key);
  } else if (value is bool) {
    await prefs.setBool(key, value);
  } else if (value is String) {
    await prefs.setString(key, value);
  } else if (value is double) {
    await prefs.setDouble(key, value);
  }
}
