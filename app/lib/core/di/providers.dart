// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/api_client.dart';
import '../api/sse_client.dart';
import '../config/app_config.dart';
import '../config/connection_settings.dart';

/// Configuration de connexion effective : persistée via
/// `connectionSettingsProvider` (surchargeable pour les tests).
final appConfigProvider = Provider<AppConfig>(
  (ref) => ref.watch(connectionSettingsProvider).value ?? const AppConfig(),
);

/// Client REST unique.
final apiClientProvider = Provider<ApiClient>((ref) {
  final client = ApiClient(ref.watch(appConfigProvider));
  ref.onDispose(client.close);
  return client;
});

/// Flux SSE `/api/events` — démarré une fois, partagé par broadcast.
final sseClientProvider = Provider<SseClient>((ref) {
  final client = SseClient(ref.watch(appConfigProvider));
  client.start();
  ref.onDispose(client.dispose);
  return client;
});

/// État du flux SSE (barre d'état « connecté au daemon »).
final sseConnectedProvider = StreamProvider<bool>((ref) {
  return ref.watch(sseClientProvider).connected;
});

/// Événements du daemon — chaque feature filtre les topics qui
/// l'intéressent et invalide ses données en conséquence.
final daemonEventsProvider = StreamProvider<SseEvent>((ref) {
  return ref.watch(sseClientProvider).events;
});

/// Watchdog connexion : tant que le flux SSE est coupé, re-résout
/// l'URL/clé du daemon toutes les 5 s (`configuration.json` peut
/// pointer un port `http_port_running` périmé si le daemon a
/// redémarré entre le `build()` et le bind — l'app restait figée sur
/// le mauvais port avec le backoff SSE jusqu'à 30 s).
final connectionWatchdogProvider = Provider<void>((ref) {
  Timer? timer;
  ref.listen<AsyncValue<bool>>(sseConnectedProvider, (prev, next) {
    final connected = next.value ?? false;
    if (connected) {
      timer?.cancel();
      timer = null;
      return;
    }
    timer ??= Timer.periodic(const Duration(seconds: 5), (_) {
      ref.read(connectionSettingsProvider.notifier).rediscover();
    });
  });
  ref.onDispose(() => timer?.cancel());
});

/// Battement périodique pour les providers à sondage (compteurs de
/// circuits, statistiques…).
final tickProvider = StreamProvider.autoDispose.family<int, Duration>(
  (ref, period) => Stream.periodic(period, (i) => i),
);

/// Requête de recherche globale (champ de la barre du haut) — la page
/// `/search` la consomme ; vide = afficher les torrents populaires.
final searchQueryProvider = NotifierProvider<SearchQueryNotifier, String>(
  SearchQueryNotifier.new,
);

class SearchQueryNotifier extends Notifier<String> {
  @override
  String build() => '';

  void set(String value) => state = value;
}

/// Fichiers `.torrent`/`.magnet` passés en argv au lancement
/// (« Ouvrir avec » / association Windows). Surchargé dans
/// `main()` via `ProviderScope(overrides: …)`.
final startupFilesProvider = Provider<List<String>>((ref) => const []);

/// File des fichiers à importer : argv de démarrage + glisser-
/// déposer. Chaque entrée ouvre le dialogue « Ajouter » à tour
/// de rôle (`PendingFilesHandler` dans `app_shell.dart`).
final pendingFilesProvider =
    NotifierProvider<PendingFilesNotifier, List<String>>(
      PendingFilesNotifier.new,
    );

class PendingFilesNotifier extends Notifier<List<String>> {
  @override
  List<String> build() => List.of(ref.watch(startupFilesProvider));

  void enqueue(Iterable<String> paths) => state = [...state, ...paths];

  /// Retire le premier élément (après fermeture du dialogue).
  void pop() => state = state.sublist(1);
}
