// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart' show Override;
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/app.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/config/connection_settings.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/downloads/domain/download.dart';
import 'package:onionbit_ui/features/downloads/presentation/providers/downloads_providers.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_attachment.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_contact.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_conversation.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_message.dart';
import 'package:onionbit_ui/features/messaging/presentation/pages/messaging_page.dart';
import 'package:onionbit_ui/features/messaging/presentation/providers/messaging_providers.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/pages/settings_page.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../helpers/l10n.dart';

/// Golden par palier de breakpoint des écrans à fort trafic
/// (ADR-0021 §9, étape 77) : la coquille adaptative + Téléchargements
/// en compact/medium/expanded, Messagerie en compact/expanded,
/// Réglages en expanded. Toute dérive visuelle du nouveau design
/// system casse une image avant de casser l'œil — régénérer après un
/// changement volontaire : `flutter test --update-goldens`.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  // -- Fakes déterministes (aucune animation ni I/O) ----------------

  const conv = MessagingConversation(
    convId: 'aabbccdd00112233',
    kind: 'direct',
    name: '',
    state: 'active',
    createdAt: 1,
    unread: 0,
    lastTs: 0,
    peer: 'deadbeefdeadbeef',
    alias: 'Alice',
  );

  List<Override> shellOverrides() => [
    sseClientProvider.overrideWithValue(_FakeSse()),
    downloadsProvider.overrideWith(_EmptyDownloads.new),
    searchResultsProvider.overrideWith(_EmptySearch.new),
    remoteResultsProvider.overrideWith(_EmptyRemote.new),
    anonLaneProvider.overrideWith(
      (ref) async => AnonLaneStatus.disabled,
    ),
  ];

  List<Override> messagingOverrides() => [
    messagingEnabledProvider.overrideWith((ref) async => true),
    messagingStatsProvider.overrideWith((ref) async => null),
    messagingContactsProvider.overrideWith(
      (ref) async => const <MessagingContact>[],
    ),
    messagingPendingProvider.overrideWith(
      (ref) async => const <MessagingContact>[],
    ),
    messagingConversationsProvider.overrideWith((ref) async => [conv]),
    messagingEventsProvider.overrideWith((ref) => const Stream.empty()),
    messagingEventsBridgeProvider.overrideWith((ref) {}),
    convHistoryProvider.overrideWith(
      (ref, convId) async => const <MessagingMessage>[],
    ),
    convAttachmentsProvider.overrideWith(
      (ref, convId) async => const <MessagingAttachment>[],
    ),
  ];

  Future<void> pumpApp(WidgetTester tester, Size size) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      ProviderScope(
        overrides: shellOverrides(),
        child: const OnionbitApp(),
      ),
    );
    await tester.pump();
    await tester.pump();
  }

  Future<void> pumpIsolated(
    WidgetTester tester,
    Widget page,
    Size size,
    List<Override> overrides,
  ) async {
    SharedPreferences.setMockInitialValues({});
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      ProviderScope(
        overrides: overrides,
        child: l10nTestApp(Scaffold(body: page)),
      ),
    );
    await tester.pump();
    await tester.pump();
  }

  const compact = Size(390, 844); // téléphone
  const medium = Size(800, 600); // tablette compacte / fenêtre réduite
  const expanded = Size(1440, 900); // bureau

  group('coquille + téléchargements', () {
    testWidgets('compact', (tester) async {
      await pumpApp(tester, compact);
      await expectLater(
        find.byType(OnionbitApp),
        matchesGoldenFile('goldens/shell_downloads_compact.png'),
      );
    });
    testWidgets('medium', (tester) async {
      await pumpApp(tester, medium);
      await expectLater(
        find.byType(OnionbitApp),
        matchesGoldenFile('goldens/shell_downloads_medium.png'),
      );
    });
    testWidgets('expanded', (tester) async {
      await pumpApp(tester, expanded);
      await expectLater(
        find.byType(OnionbitApp),
        matchesGoldenFile('goldens/shell_downloads_expanded.png'),
      );
    });
  });

  group('messagerie', () {
    testWidgets('compact — liste seule', (tester) async {
      await pumpIsolated(
        tester,
        const MessagingPage(),
        compact,
        messagingOverrides(),
      );
      await expectLater(
        find.byType(MessagingPage),
        matchesGoldenFile('goldens/messaging_compact.png'),
      );
    });
    testWidgets('expanded — liste + détail', (tester) async {
      await pumpIsolated(
        tester,
        const MessagingPage(),
        expanded,
        messagingOverrides(),
      );
      await expectLater(
        find.byType(MessagingPage),
        matchesGoldenFile('goldens/messaging_expanded.png'),
      );
    });
  });

  group('réglages', () {
    testWidgets('expanded — catégories', (tester) async {
      await pumpIsolated(
        tester,
        const SettingsPage(),
        expanded,
        [
          settingsRepositoryProvider.overrideWithValue(_FakeSettings()),
          connectionSettingsProvider.overrideWith(_FakeConn.new),
          anonLaneProvider.overrideWith(
            (ref) async => AnonLaneStatus.disabled,
          ),
        ],
      );
      await expectLater(
        find.byType(SettingsPage),
        matchesGoldenFile('goldens/settings_expanded.png'),
      );
    });
  });
}

class _FakeSse extends SseClient {
  _FakeSse() : super(const AppConfig());
  @override
  void start() {}
}

class _EmptyDownloads extends DownloadsNotifier {
  @override
  Future<List<Download>> build() async => [];
}

class _EmptySearch extends SearchResultsNotifier {
  @override
  Future<List<TorrentResult>> build() async => [];
}

class _EmptyRemote extends RemoteResultsNotifier {
  @override
  RemoteResults build() => const RemoteResults(
    state: RemoteSearchState(uuid: null, peerCount: 0),
    results: [],
  );
}

class _FakeConn extends ConnectionSettingsNotifier {
  @override
  Future<AppConfig> build() async => const AppConfig();
}

/// Dépôt réglages minimal — suffisant pour rendre la page (les
/// sections sans provider dédié montrent leur état de chargement/erreur,
/// déterministe).
class _FakeSettings implements SettingsRepository {
  @override
  Future<Map<String, dynamic>> get() async => {};
  @override
  Future<void> update(Map<String, dynamic> settings) async {}
  @override
  Future<void> shutdown() async {}
  @override
  Future<Map<String, int>> dirSpace({String? directory, String? area}) async =>
      {'total': 0, 'used': 0, 'free': 0};
  @override
  Future<List<Map<String, dynamic>>> rssItems() async => [];
  @override
  Future<void> setRssFeeds(List<String> urls) async {}
  @override
  Future<Map<String, dynamic>> versions() async => {};
  @override
  Future<Map<String, dynamic>> checkVersion() async => {};
  @override
  Future<String?> identityPublicKey() async => null;
  @override
  Future<Map<String, dynamic>> identityStatus() async => {};
  @override
  Future<String?> identityRecoveryPhrase({String? lang}) async => null;
  @override
  Future<Map<String, dynamic>> identityExport({String? password}) async => {};
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
