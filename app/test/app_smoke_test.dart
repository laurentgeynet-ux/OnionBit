// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/app.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/downloads/domain/download.dart';
import 'package:onionbit_ui/features/downloads/presentation/providers/downloads_providers.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
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

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('le shell affiche la sidebar et la page téléchargements', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          sseClientProvider.overrideWithValue(_FakeSseClient()),
          downloadsProvider.overrideWith(_EmptyDownloads.new),
          searchResultsProvider.overrideWith(_EmptySearch.new),
          remoteResultsProvider.overrideWith(_EmptyRemote.new),
          anonLaneProvider.overrideWith((ref) async => AnonLaneStatus.disabled),
        ],
        child: const OnionbitApp(),
      ),
    );
    await tester.pump();
    await tester.pump();

    expect(find.text('Add'), findsWidgets);
    expect(find.text('Downloads'), findsWidgets);
    expect(find.text('Search'), findsOneWidget);
    expect(find.text('Diagnostics'), findsOneWidget);
    expect(find.text('Aucun téléchargement'), findsOneWidget);
  });
}
