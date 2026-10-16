// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';
import 'package:onionbit_ui/features/settings/presentation/widgets/network_section.dart';

import 'helpers/l10n.dart';

/// Dépôt fake — `patches` enregistre chaque `update` (le toggle Tor
/// sauvegarde immédiatement via `SettingsSwitch`).
class _Fake implements SettingsRepository {
  final List<Map<String, dynamic>> patches = [];

  @override
  Future<Map<String, dynamic>> get() async => {
    'libtorrent': {
      'port': 0,
      'proxy_type': 0,
      'proxy_server': '',
      'proxy_auth': '',
    },
    'ipv8': {'enabled': true},
    'tor': {
      'enabled': false,
      'mode': 'embedded',
      'external_socks5': 'socks5h://127.0.0.1:9050',
      // `allowed_hosts` vide volontairement : l'hôte affiché provient
      // du miroir du backend (union effective de TorTargetPolicy).
      'allowed_hosts': const <String>[],
      'search': {
        'backends': {
          'tpb': {
            'max_results': 100,
            'mirrors': const [
              'piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion',
            ],
          },
        },
      },
    },
  };

  @override
  Future<void> update(Map<String, dynamic> settings) async =>
      patches.add(settings);

  @override
  dynamic noSuchMethod(Invocation i) =>
      Future<Map<String, dynamic>>.value(const {});
}

void main() {
  Future<void> pump(WidgetTester tester, _Fake repo) async {
    tester.view.physicalSize = const Size(900, 2400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [settingsRepositoryProvider.overrideWithValue(repo)],
        child: l10nTestApp(
          const Scaffold(body: SingleChildScrollView(child: NetworkSection())),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('sous-section Tor : toggle, mode, allowlist', (tester) async {
    final repo = _Fake();
    await pump(tester, repo);
    expect(tester.takeException(), isNull);

    // Sous-section présente : toggle « Enable Tor », sélecteur de mode,
    // allowlist et backend en lecture seule.
    expect(find.text('Tor connectivity'), findsOneWidget);
    expect(find.text('Enable Tor'), findsOneWidget);
    expect(find.text('Embedded (Arti)'), findsOneWidget);
    expect(find.text('tpb'), findsOneWidget);
    expect(find.textContaining('piratebayo3klnzok'), findsOneWidget);

    // Le toggle sauvegarde immédiatement `tor.enabled`.
    await tester.tap(find.text('Enable Tor'));
    await tester.pumpAndSettle();
    expect(repo.patches, [
      {
        'tor': {'enabled': true},
      },
    ]);

    // Champ SOCKS5 caché en mode embedded, révélé en mode external.
    expect(find.text('SOCKS5 endpoint'), findsNothing);
    await tester.tap(find.byType(DropdownButtonFormField<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('External Tor (SOCKS5)').last);
    await tester.pumpAndSettle();
    expect(find.text('SOCKS5 endpoint'), findsOneWidget);
  });
}
