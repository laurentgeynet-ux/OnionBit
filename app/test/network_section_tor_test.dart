// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_repository.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
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

/// Dépôt diagnostic fake — `launched` enregistre chaque URL passée à
/// `POST /api/browser/launch` ; `torStatus` expose un proxy navigateur
/// actif pour couvrir la pastille `conns_active`.
class _FakeDiag implements DiagnosticRepository {
  final List<String> launched = [];

  @override
  Future<BrowserLaunchResult> launchBrowser(
    String url, {
    bool embed = false,
  }) async {
    launched.add(url);
    return (pid: 4242, hwnd: null);
  }

  @override
  Future<TorRuntimeStatus> torStatus() async => const TorRuntimeStatus(
    enabled: true,
    mode: 'embedded',
    state: TorRuntimeState.ready,
    progress: 100,
    browserProxy: BrowserProxyStatus(
      enabled: true,
      listen: '127.0.0.1:54321',
      connsActive: 3,
    ),
  );

  @override
  dynamic noSuchMethod(Invocation i) =>
      Future<Map<String, dynamic>>.value(const {});
}

void main() {
  Future<void> pump(WidgetTester tester, _Fake repo, [_FakeDiag? diag]) async {
    tester.view.physicalSize = const Size(900, 2600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(repo),
          diagnosticRepositoryProvider.overrideWithValue(diag ?? _FakeDiag()),
        ],
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
    // Adresse onion v3 intégrale et sélectionnable (SelectableText =
    // RichText interne — `findRichText` requis pour la retrouver).
    expect(
      find.textContaining(
        'piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion',
        findRichText: true,
      ),
      findsOneWidget,
    );

    // Le toggle sauvegarde immédiatement `tor.enabled`.
    await tester.tap(find.text('Enable Tor'));
    await tester.pumpAndSettle();
    expect(repo.patches, [
      {
        'tor': {'enabled': true},
      },
    ]);

    // Champ SOCKS5 caché en mode embedded, révélé en mode external —
    // le premier dropdown String est le mode Tor (le second est la
    // politique de sortie clearnet du navigateur, ADR-0030).
    expect(find.text('SOCKS5 endpoint'), findsNothing);
    await tester.tap(find.byType(DropdownButtonFormField<String>).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('External Tor (SOCKS5)').last);
    await tester.pumpAndSettle();
    expect(find.text('SOCKS5 endpoint'), findsOneWidget);
  });

  testWidgets('sous-section Navigateur : proxy, exit, binaire, lancement', (
    tester,
  ) async {
    final repo = _Fake();
    final diag = _FakeDiag();
    await pump(tester, repo, diag);
    expect(tester.takeException(), isNull);

    // Sous-section ADR-0030 : titre, toggle proxy, politique de
    // sortie tri-state honnête, champ binaire, pastille conns_active.
    expect(find.text('Browser (.onion)'), findsOneWidget);
    expect(find.text('Browser connection'), findsOneWidget);
    expect(find.text('Clearnet exit via Tor'), findsOneWidget);
    expect(find.text('Browser binary'), findsOneWidget);
    expect(find.text('proxy 127.0.0.1:54321'), findsOneWidget);
    expect(find.text('3 active'), findsOneWidget);

    // Le toggle proxy sauvegarde immédiatement
    // `tor.browser_proxy.enabled` (hot-swap de la pile Tor).
    await tester.tap(find.text('Browser connection'));
    await tester.pumpAndSettle();
    expect(repo.patches, [
      {
        'tor': {
          'browser_proxy': {'enabled': true},
        },
      },
    ]);

    // Politique de sortie : second dropdown String, hint honnête du
    // mode choisi (l'exit voit la destination en https-only).
    await tester.tap(find.byType(DropdownButtonFormField<String>).last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('HTTPS only (:443)').last);
    await tester.pumpAndSettle();
    expect(
      find.textContaining('the Tor exit sees the destination'),
      findsOneWidget,
    );

    // Un hôte .onion de l'allowlist est actionnable — icône open_in_new
    // → POST /api/browser/launch sur http://<hôte>/. On invoque le
    // onTap de l'InkWell : la cible tactile (13px dans un Chip) est un
    // détail de layout, le contrat testé est URL + snackbar.
    final launch = tester.widget<InkWell>(
      find
          .ancestor(
            of: find.byIcon(Icons.open_in_new).first,
            matching: find.byType(InkWell),
          )
          .first,
    );
    launch.onTap!();
    await tester.pumpAndSettle();
    expect(diag.launched, [
      'http://piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion/',
    ]);
    expect(find.text('Browser launched (pid 4242)'), findsOneWidget);
  });
}
