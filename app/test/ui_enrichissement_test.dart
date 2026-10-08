// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/core/config/ui_prefs.dart';
import 'package:onionbit_ui/core/layout/app_sidebar.dart';
import 'package:onionbit_ui/core/notifications/app_notification.dart';
import 'package:onionbit_ui/core/notifications/notifications_provider.dart';
import 'package:onionbit_ui/core/widgets/empty_state.dart';
import 'package:onionbit_ui/features/downloads/presentation/providers/downloads_providers.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('centre de notifications', () {
    test('push ajoute en tête, compteur non-lues, borne à 100', () {
      final c = ProviderContainer();
      addTearDown(c.dispose);
      final n = c.read(notificationsProvider.notifier);

      for (var i = 0; i < 105; i++) {
        n.push(AppNotification(title: 'n$i', message: ''));
      }
      expect(c.read(notificationsProvider).length, 100);
      expect(c.read(notificationsProvider).first.title, 'n104');
      expect(c.read(unreadNotificationsProvider), 100);

      n.markAllRead();
      expect(c.read(unreadNotificationsProvider), 0);

      n.clear();
      expect(c.read(notificationsProvider), isEmpty);
    });
  });

  group('préférences UI persistées', () {
    test('le tri Téléchargements est écrit puis rechargé', () async {
      SharedPreferences.setMockInitialValues({});
      final c = ProviderContainer();
      addTearDown(c.dispose);

      c.read(downloadSortProvider.notifier).tap(DownloadSort.size);
      await Future<void>.delayed(Duration.zero);

      final c2 = ProviderContainer();
      addTearDown(c2.dispose);
      await c2.read(uiPrefsInitProvider.future);
      final sort = c2.read(downloadSortProvider);
      expect(sort.col, DownloadSort.size);
      expect(sort.asc, isTrue);
    });

    test('rail rétracté et filtres repliés sont rechargés', () async {
      SharedPreferences.setMockInitialValues({
        'ui.sidebarCollapsed': true,
        'ui.filtersExpanded': false,
        'ui.downloadViewMode': 'grid',
      });
      final c = ProviderContainer();
      addTearDown(c.dispose);
      await c.read(uiPrefsInitProvider.future);

      expect(c.read(sidebarCollapsedProvider), isTrue);
      expect(c.read(sidebarFiltersExpandedProvider), isFalse);
      expect(c.read(downloadViewModeProvider), DownloadViewMode.grid);
    });

    test('tri Rechercher persisté (chaîne vide = pertinence)', () async {
      SharedPreferences.setMockInitialValues({'ui.searchColSort': ''});
      final c = ProviderContainer();
      addTearDown(c.dispose);
      await c.read(uiPrefsInitProvider.future);
      expect(c.read(searchColSortProvider), isNull);

      c.read(searchColSortProvider.notifier).tap(SearchCol.seeds);
      await Future<void>.delayed(Duration.zero);
      final p = await SharedPreferences.getInstance();
      expect(p.getString('ui.searchColSort'), 'seeds:asc');
    });
  });

  group('widgets partagés', () {
    testWidgets('EmptyState affiche icône, titre et action', (tester) async {
      var tapped = false;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: EmptyState(
              icon: Icons.download_outlined,
              title: 'Aucun téléchargement',
              message: 'Ajoutez un magnet.',
              action: FilledButton(
                onPressed: () => tapped = true,
                child: const Text('Ajouter'),
              ),
            ),
          ),
        ),
      );
      expect(find.text('Aucun téléchargement'), findsOneWidget);
      await tester.tap(find.text('Ajouter'));
      expect(tapped, isTrue);
    });
  });
}
