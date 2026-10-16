// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:onionbit_ui/core/api/sse_client.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/di/providers.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';
import 'package:onionbit_ui/features/downloads/domain/download.dart';
import 'package:onionbit_ui/features/downloads/presentation/providers/downloads_providers.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';
import 'package:onionbit_ui/features/search/presentation/pages/search_page.dart';
import 'package:onionbit_ui/features/search/presentation/providers/search_providers.dart';

import 'helpers/l10n.dart';

class _FakeSseClient extends SseClient {
  _FakeSseClient() : super(const AppConfig());

  @override
  void start() {} // Pas de connexion réseau en test.
}

class _FixedDownloads extends DownloadsNotifier {
  _FixedDownloads(this.items);

  final List<Download> items;

  @override
  Future<List<Download>> build() async => items;
}

class _FixedSearch extends SearchResultsNotifier {
  _FixedSearch(this.items);

  final List<TorrentResult> items;

  @override
  Future<List<TorrentResult>> build() async => items;
}

class _EmptyRemote extends RemoteResultsNotifier {
  @override
  RemoteResults build() => const RemoteResults(
    state: RemoteSearchState(uuid: null, peerCount: 0),
    results: [],
  );
}

Download _dl({
  required String infohash,
  required String status,
  required double progress,
  String storageArea = 'public',
  String destination = 'C:/data/public/ubuntu.iso',
}) => Download(
  infohash: infohash,
  name: 'ubuntu.iso',
  progress: progress,
  size: 1024,
  speedDown: 0,
  speedUp: 0,
  status: status,
  statusCode: 0,
  etaSeconds: 0,
  numSeeds: 0,
  numPeers: 0,
  numConnectedPeers: 0,
  hops: 0,
  anonDownload: false,
  safeSeeding: false,
  uploaded: 0,
  downloaded: 0,
  ratio: 0,
  error: '',
  destination: destination,
  streamable: false,
  queuePosition: -1,
  autoManaged: false,
  userStopped: false,
  uploadLimit: 0,
  downloadLimit: 0,
  seedingRatio: 0,
  timeAdded: 0,
  timeFinished: 0,
  trackers: const [],
  peers: const [],
  storageArea: storageArea,
);

TorrentResult _res(String infohash, String name) => TorrentResult(
  infohash: infohash,
  name: name,
  size: 1024,
  source: TorrentSource.local,
  seeders: 5,
  leechers: 1,
);

Widget _app({required List<Download> downloads}) {
  return ProviderScope(
    overrides: [
      sseClientProvider.overrideWithValue(_FakeSseClient()),
      downloadsProvider.overrideWith(() => _FixedDownloads(downloads)),
      searchResultsProvider.overrideWith(
        () => _FixedSearch([
          _res('ih-seed-priv', 'Ubuntu (seed privé)'),
          _res('ih-dl-pub', 'Debian (public)'),
          _res('ih-unknown', 'Inconnu'),
        ]),
      ),
      remoteResultsProvider.overrideWith(_EmptyRemote.new),
      torStatusProvider.overrideWith((ref) async => TorRuntimeStatus.disabled),
    ],
    child: l10nTestApp(const Scaffold(body: SearchPage())),
  );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'résultat déjà téléchargé : statut réel (Partage) et non « In progress »',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      tester.view.physicalSize = const Size(1400, 900);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      await tester.pumpWidget(
        _app(
          downloads: [
            _dl(
              infohash: 'ih-seed-priv',
              status: 'SEEDING',
              progress: 1.0,
              storageArea: 'private',
              destination: '@private/downloads/ubuntu.iso',
            ),
            _dl(infohash: 'ih-dl-pub', status: 'DOWNLOADING', progress: 0.42),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      // Même libellé que l'onglet Téléchargements — fini « In progress ».
      expect(find.text('Seeding 100 %'), findsOneWidget);
      expect(find.text('Downloading 42 %'), findsOneWidget);
      expect(find.text('In progress'), findsNothing);
      // Badge zone privée (clé chiffrement) sur le résultat privé.
      expect(find.byIcon(Icons.enhanced_encryption_outlined), findsOneWidget);
      // Résultat inconnu : bouton Ajouter toujours présent.
      expect(find.byIcon(Icons.download), findsWidgets);
    },
  );

  testWidgets(
    'menu contextuel : fichier public = Ouvrir + Déplacer ; privé = Déplacer seul',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      tester.view.physicalSize = const Size(1400, 900);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      await tester.pumpWidget(
        _app(
          downloads: [
            _dl(
              infohash: 'ih-seed-priv',
              status: 'SEEDING',
              progress: 1.0,
              storageArea: 'private',
              destination: '@private/downloads/ubuntu.iso',
            ),
            _dl(infohash: 'ih-dl-pub', status: 'SEEDING', progress: 1.0),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      // Résultat privé : « Ouvrir » absent (spec `@private/` non
      // ouvrable), « Déplacer » présent (extraction = déchiffrement).
      await tester.tap(
        find.text('Ubuntu (seed privé)'),
        buttons: kSecondaryMouseButton,
      );
      await tester.pump();
      await tester.pump();
      expect(find.text('Move storage…'), findsOneWidget);
      expect(find.text('Open folder'), findsNothing);
      await tester.tapAt(const Offset(10, 10));
      await tester.pump();
      await tester.pump();

      // Résultat public : les deux actions locales présentes.
      await tester.tap(
        find.text('Debian (public)'),
        buttons: kSecondaryMouseButton,
      );
      await tester.pump();
      await tester.pump();
      expect(find.text('Move storage…'), findsOneWidget);
      expect(find.text('Open folder'), findsOneWidget);
    },
  );
}
