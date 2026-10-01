// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/pages/settings_page.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';

class _FakeSettings implements SettingsRepository {
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
  Future<Map<String, int>> dirSpace({String? directory}) async =>
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
        child: const MaterialApp(home: Scaffold(body: SettingsPage())),
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

    expect(tester.takeException(), isNull);
    expect(find.text('Daemon'), findsWidgets);
  });
}
