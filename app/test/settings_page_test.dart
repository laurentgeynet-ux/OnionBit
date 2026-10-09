// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/pages/settings_page.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';
import 'package:onionbit_ui/features/settings/presentation/widgets/daemon_section.dart';
import 'package:onionbit_ui/features/settings/presentation/widgets/identity_section.dart';

import 'helpers/l10n.dart';

class _FakeSettings implements SettingsRepository {
  _FakeSettings({this.phrase});

  /// Phrase retournée par `identityRecoveryPhrase` — `null` simule
  /// une identité legacy / erreur daemon (chemin « indisponible »).
  final String? phrase;

  @override
  Future<Map<String, dynamic>> get() async => {
    'libtorrent': {
      'max_download_rate': 0,
      'max_upload_rate': 0,
      'active_downloads': 3,
      'active_seeds': 5,
      'active_checking': 1,
      'active_limit': 500,
      'download_defaults': {'saveas': '/tmp', 'auto_managed': false},
      'dht': true,
      'upnp': true,
      'natpmp': true,
      'lsd': true,
      'utp': true,
      'proxy_type': 0,
      'proxy_server': '',
      'proxy_auth': false,
      'proxy_username': '',
      'proxy_password': '',
    },
    'tunnel_community': {
      'min_circuits': 1,
      'max_circuits': 8,
      'exitnode_enabled': false,
    },
    'download_defaults': {'number_anon_downloads': 1, 'seeding_ratio': 2.0},
    'ipv8': {'enabled': true, 'bootstrap_override': ''},
    'watch_folder': {'enabled': false, 'directory': ''},
    'rss': {'enabled': false, 'urls': <String>[]},
    'api': {'http_port': 8085},
    'chant': {'enabled': true},
    'metadata_store': {'enabled': true},
    'torrent_checking': {'enabled': true},
  };

  @override
  Future<void> update(Map<String, dynamic> settings) async {}
  @override
  Future<void> shutdown() async {}
  @override
  Future<Map<String, int>> dirSpace({String? directory, String? area}) async =>
      {'total': 100, 'used': 50, 'free': 50};
  @override
  Future<List<Map<String, dynamic>>> rssItems() async => [];
  @override
  Future<void> setRssFeeds(List<String> urls) async {}
  @override
  Future<Map<String, dynamic>> versions() async => {'current': '0.3.0'};
  @override
  Future<Map<String, dynamic>> checkVersion() async =>
      {'has_version': false};
  @override
  Future<String?> identityPublicKey() async => 'aabbcc';
  @override
  Future<Map<String, dynamic>> identityStatus() async => {
    'state': 'ready',
    'seeded': true,
    'mode': 'persistent',
    'persistent': true,
    'public_key': 'aabbcc',
  };
  @override
  Future<String?> identityRecoveryPhrase({String? lang}) async => phrase;
  @override
  Future<Map<String, dynamic>> identityExport({String? password}) async =>
      {'key': 'deadbeef'};
  @override
  Future<void> identityRestore(
    String keyHex, {
    String? password,
    bool forceLegacy = false,
  }) async {}
  @override
  Future<void> identityRestorePhrase(String phrase) async {}
  @override
  Future<void> identitySetAtRest({
    required bool enabled,
    required String password,
  }) async {}
  @override
  Future<void> identityCreate({String? password}) async {}
  @override
  Future<void> identityGuest() async {}
  @override
  Future<void> identityUnlock(String password) async {}
  @override
  Future<Map<String, dynamic>> privateZone() async => {
    'state': 'mounted',
    'downloads': const [],
    'orphans': const {'obd_groups': 0, 'bitv': 0},
  };
  @override
  Future<void> purgePrivateOrphans() async {}
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('la page Réglages rend toutes les sections sans erreur', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(1400, 3000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(_FakeSettings()),
        ],
        child: l10nTestApp(const Scaffold(body: SettingsPage())),
      ),
    );
    await tester.pumpAndSettle();

    // Défile jusqu'en bas — toute section qui lève une exception de
    // build/layout se manifeste ici (ErrorWidget / exception test).
    await tester.drag(
      find.byType(ListView).last,
      const Offset(0, -2000),
    );
    await tester.pumpAndSettle();
    await tester.drag(
      find.byType(ListView).last,
      const Offset(0, -2000),
    );
    await tester.pumpAndSettle();
    await tester.drag(
      find.byType(ListView).last,
      const Offset(0, -2000),
    );
    await tester.pumpAndSettle();
    // Les en-têtes de catégorie (étape 75) ont allongé la page —
    // défile jusqu'à la dernière section (« Daemon state » de
    // `DaemonSection`, tout en bas) peu importe sa hauteur.
    // `dragUntilVisible` cible la ListView (hit-test du scrollable) —
    // `byType(Scrollable)` attraperait les scrollables internes des
    // champs de saisie (`restorationId: 'editable'`).
    await tester.dragUntilVisible(
      find.text('Daemon state'),
      find.byType(ListView).last,
      const Offset(0, -400),
    );

    expect(tester.takeException(), isNull);
    expect(find.text('Daemon state'), findsWidgets);
  });

  testWidgets('catégories repliables : repli et deep-link ?s=', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = const Size(1400, 3000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(_FakeSettings()),
        ],
        child: l10nTestApp(
          const Scaffold(body: SettingsPage(sectionId: 'daemon')),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Deep-link `?s=daemon` : la page défile jusqu'à la section cible
    // (catégorie « Avancé », tout en bas) sans exception — la section
    // n'est montée qu'une fois le viewport atteint (ListView
    // paresseuse), sa présence prouve le défilement.
    expect(tester.takeException(), isNull);
    expect(find.byType(DaemonSection), findsOneWidget);

    // Les en-têtes de catégorie (étape 75 — petites capitales) sont
    // joignables par défilement.
    await tester.dragUntilVisible(
      find.text('ADVANCED'),
      find.byType(ListView).last,
      const Offset(0, 400),
    );
    await tester.dragUntilVisible(
      find.text('ESSENTIALS'),
      find.byType(ListView).last,
      const Offset(0, 400),
    );

    // Replier « Essentiels » retire ses sections de l'arbre — le
    // chevron bascule d'`expand_less` à `expand_more`.
    expect(find.byIcon(Icons.expand_more), findsNothing);
    await tester.tap(find.text('ESSENTIALS'));
    await tester.pumpAndSettle();
    expect(find.byIcon(Icons.expand_more), findsOneWidget);

    // L'en-tête voisin reste joignable — le regroupement n'a pas
    // cassé le défilement.
    await tester.dragUntilVisible(
      find.text('NETWORK & ANONYMITY'),
      find.byType(ListView).last,
      const Offset(0, -400),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'phrase de récupération : repository null → « indisponible », pas de spinner infini',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            settingsRepositoryProvider.overrideWithValue(_FakeSettings()),
          ],
          child: l10nTestApp(const Scaffold(body: IdentitySection())),
        ),
      );
      await tester.pumpAndSettle();

      // Le bouton existe en double (carte de rappel + rangée
      // d'actions) — les deux ouvrent le même dialogue.
      await tester.tap(find.text('Show recovery phrase').first);
      await tester.pumpAndSettle();

      // `identityRecoveryPhrase → null` doit retomber sur le message
      // dédié — un `!snap.hasData` laisserait le spinner tourner
      // indéfiniment (hasData == data != null).
      expect(
        find.text(
          'No recovery phrase — this is a legacy identity; use key export instead.',
        ),
        findsOneWidget,
      );
      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('phrase de récupération : 24 mots numérotés affichés', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final words = List.generate(24, (i) => 'mot$i').join(' ');
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(
            _FakeSettings(phrase: words),
          ),
        ],
        child: l10nTestApp(const Scaffold(body: IdentitySection())),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('Show recovery phrase').first);
    await tester.pumpAndSettle();

    expect(find.text('1. mot0'), findsOneWidget);
    expect(find.text('24. mot23'), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
