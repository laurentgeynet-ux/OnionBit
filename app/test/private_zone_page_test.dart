// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/private_zone/presentation/pages/private_zone_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'helpers/l10n.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const ih = 'aabbccddeeff00112233445566778899aabbccdd';

  /// Daemon factice : catalogue à une entrée, DELETE enregistré.
  MockClient daemon({required List<String> deleted}) {
    return MockClient((req) async {
      if (req.method == 'GET' && req.url.path == '/api/private') {
        return http.Response(
          jsonEncode({
            'state': 'mounted',
            'downloads': [
              {
                'infohash': ih,
                'name': 'mes-docs',
                'destination': '@private/downloads',
                'paused': true,
                'time_added': 1700000000,
              },
            ],
            'orphans': {'obd_groups': 0, 'bitv': 0},
          }),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      if (req.method == 'DELETE' && req.url.path == '/api/private/$ih') {
        deleted.add(ih);
        return http.Response(
          '{"deleted": true}',
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('{}', 404);
    });
  }

  Widget app(MockClient client) => ProviderScope(
    overrides: [
      apiClientProvider.overrideWithValue(
        ApiClient(const AppConfig(), httpClient: client),
      ),
    ],
    child: l10nTestApp(const Scaffold(body: PrivateZonePage())),
  );

  testWidgets(
    'zone privée : entrée du catalogue, menu Importer, suppression confirmée',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final deleted = <String>[];

      await tester.pumpWidget(app(daemon(deleted: deleted)));
      await tester.pump();
      await tester.pump();

      // Catalogue déplié : l'entrée manifeste est visible.
      expect(find.text('Private zone'), findsOneWidget);
      expect(find.text('mes-docs'), findsOneWidget);

      // Bouton Importer de l'en-tête : menu fichiers / dossier.
      await tester.tap(find.byIcon(Icons.input_outlined));
      await tester.pumpAndSettle();
      expect(find.text('Import files…'), findsOneWidget);
      expect(find.text('Import folder…'), findsOneWidget);
      // Fermeture sans choisir (les pickers natifs ne sont pas testables).
      await tester.tapAt(const Offset(5, 5));
      await tester.pumpAndSettle();

      // Menu de l'entrée → « Supprimer » → popup de confirmation.
      await tester.tap(find.byIcon(Icons.more_vert));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Delete').last);
      await tester.pumpAndSettle();
      expect(find.text('Delete this entry?'), findsOneWidget);
      expect(deleted, isEmpty, reason: 'pas de DELETE avant confirmation');

      // Confirmation → DELETE /api/private/{key}.
      await tester.tap(find.widgetWithText(FilledButton, 'Delete'));
      await tester.pumpAndSettle();
      expect(deleted, [ih]);
    },
  );
}
