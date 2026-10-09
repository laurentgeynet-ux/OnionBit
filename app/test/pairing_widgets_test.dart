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
import 'package:onionbit_ui/core/config/connection_settings.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/core/pairing/pairing_sheet.dart';
import 'package:onionbit_ui/core/pairing/qr_scanner.dart';
import 'package:onionbit_ui/core/pairing/scan_support.dart';
import 'package:onionbit_ui/features/settings/presentation/widgets/connection_section.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'helpers/l10n.dart';

/// Daemon factice : `POST /api/pairing/token` instrumenté.
class _FakeConn extends ConnectionSettingsNotifier {
  @override
  Future<AppConfig> build() async =>
      const AppConfig(baseUrl: 'http://192.168.1.20:8085', apiKey: 'k');
}

/// Compte les émissions de jeton et répond le grant de test.
MockClient _pairingMock(void Function(http.Request) onIssue) =>
    MockClient((req) async {
      if (req.url.path == '/api/pairing/token') {
        onIssue(req);
        return http.Response(
          jsonEncode({'token': 'jeton-1234', 'expires_in_secs': 120}),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('{}', 404);
    });

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('feuille d\'appairage : QR + TTL + régénération', (
    tester,
  ) async {
    var issues = 0;
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          connectionSettingsProvider.overrideWith(_FakeConn.new),
          apiClientProvider.overrideWithValue(
            ApiClient(
              const AppConfig(),
              httpClient: _pairingMock((_) => issues++),
            ),
          ),
        ],
        child: l10nTestApp(const Scaffold(body: PairingSheet())),
      ),
    );
    // `pump` seul — le ticker 1 s empêche `pumpAndSettle` de converger.
    await tester.pump();
    await tester.pump();

    expect(find.text('Pair a mobile device'), findsOneWidget);
    expect(find.byType(QrImageView), findsOneWidget);
    expect(find.text('Expires in 120 s'), findsOneWidget);
    expect(issues, 1);

    // Le compte à rebours suit le temps écoulé.
    await tester.pump(const Duration(seconds: 3));
    expect(find.text('Expires in 117 s'), findsOneWidget);

    // Régénération : nouvelle émission côté daemon.
    await tester.tap(find.text('Regenerate token'));
    await tester.pump();
    await tester.pump();
    expect(issues, 2);
  });

  testWidgets('connexion : action « QR d\'appairage » ouvre la feuille',
      (tester) async {
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          connectionSettingsProvider.overrideWith(_FakeConn.new),
          apiClientProvider.overrideWithValue(
            ApiClient(
              const AppConfig(),
              httpClient: _pairingMock((_) {}),
            ),
          ),
        ],
        child: l10nTestApp(
          const Scaffold(body: SingleChildScrollView(
            child: ConnectionSection(),
          )),
        ),
      ),
    );
    await tester.pump();
    await tester.pump();

    // Hôte desktop de test : le bouton QR est là, le scan est masqué.
    expect(find.text('Pairing QR'), findsOneWidget);
    expect(find.text('Scan QR code'), findsNothing);

    await tester.tap(find.text('Pairing QR'));
    await tester.pump();
    await tester.pump();
    expect(find.text('Pair a mobile device'), findsOneWidget);
  });

  testWidgets('scanner : stub silencieux hors cibles caméra', (
    tester,
  ) async {
    // Hôte de test desktop : `scanPairingQr` rend null sans route,
    // les capacités sont masquées (`connection_section` s'appuie
    // dessus pour afficher/masquer les boutons).
    String? result = 'sentinelle';
    await tester.pumpWidget(
      l10nTestApp(
        Builder(
          builder: (ctx) => FilledButton(
            onPressed: () async => result = await scanPairingQr(ctx),
            child: const Text('scan'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('scan'));
    await tester.pump();
    expect(result, isNull);
    expect(canScanPairingQr, isFalse);
    expect(isMobileRemote, isFalse);
  });
}
