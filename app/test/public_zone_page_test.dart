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
import 'package:onionbit_ui/features/public_zone/presentation/pages/public_zone_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'helpers/l10n.dart';

/// Racine publique absolue du daemon factice.
const _public = 'C:/bundle/data/public';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'zone publique : liste @public, « .. » borné à la racine, navigation dans un dossier',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final browsed = <String>[];

      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            apiClientProvider.overrideWithValue(
              ApiClient(
                const AppConfig(),
                httpClient: MockClient((req) async {
                  if (req.url.path == '/api/files/browse') {
                    final p = req.url.queryParameters['path'] ?? '';
                    browsed.add(p);
                    // `@public` et sa forme absolue servent la même
                    // racine (le daemon résout le spec portable).
                    final json = {
                      '@public': {
                        'current': _public,
                        'separator': '/',
                        'paths': [
                          {'name': '..', 'path': 'C:/bundle/data', 'dir': true},
                          {
                            'name': 'downloads',
                            'path': '$_public/downloads',
                            'dir': true,
                          },
                          {
                            'name': 'readme.txt',
                            'path': '$_public/readme.txt',
                            'dir': false,
                          },
                        ],
                      },
                      _public: {
                        'current': _public,
                        'separator': '/',
                        'paths': [
                          {'name': '..', 'path': 'C:/bundle/data', 'dir': true},
                          {
                            'name': 'downloads',
                            'path': '$_public/downloads',
                            'dir': true,
                          },
                          {
                            'name': 'readme.txt',
                            'path': '$_public/readme.txt',
                            'dir': false,
                          },
                        ],
                      },
                      '$_public/downloads': {
                        'current': '$_public/downloads',
                        'separator': '/',
                        'paths': [
                          {'name': '..', 'path': _public, 'dir': true},
                          {
                            'name': 'ubuntu.iso',
                            'path': '$_public/downloads/ubuntu.iso',
                            'dir': false,
                          },
                        ],
                      },
                    }[p];
                    if (json != null) {
                      return http.Response(
                        jsonEncode(json),
                        200,
                        headers: {'content-type': 'application/json'},
                      );
                    }
                  }
                  return http.Response('{}', 404);
                }),
              ),
            ),
          ],
          child: l10nTestApp(const Scaffold(body: PublicZonePage())),
        ),
      );
      await tester.pump();
      await tester.pump();

      // La première navigation part bien du spec portable.
      expect(browsed, ['@public']);
      expect(find.text('Public zone'), findsOneWidget);
      expect(find.text('downloads'), findsOneWidget);
      expect(find.text('readme.txt'), findsOneWidget);
      // « .. » caché à la racine publique : on ne sort pas de la zone.
      expect(find.text('..'), findsNothing);

      // Descente dans le sous-dossier.
      await tester.tap(find.text('downloads'));
      await tester.pump();
      await tester.pump();
      expect(browsed.last, '$_public/downloads');
      expect(find.text('ubuntu.iso'), findsOneWidget);
      // « .. » de retour : il remonte vers la racine publique.
      expect(find.text('..'), findsOneWidget);
      await tester.tap(find.text('..'));
      await tester.pump();
      await tester.pump();
      expect(find.text('readme.txt'), findsOneWidget);
    },
  );
}
