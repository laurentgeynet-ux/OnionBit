// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/api/sse_client.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_settings_repository.dart';
import '../../domain/settings_repository.dart';

final settingsRepositoryProvider = Provider<SettingsRepository>(
  (ref) => RestSettingsRepository(ref.watch(apiClientProvider)),
);

final daemonSettingsProvider = FutureProvider.autoDispose<Map<String, dynamic>>(
  (ref) {
    // SSE `settings_changed` (POST /api/settings d'un client) →
    // recharge l'arbre : ferme la boucle entre l'éditeur avancé et
    // les sections dédiées, et entre plusieurs clients.
    ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
      if (next.value?.topic == EventTopics.settingsChanged) {
        ref.invalidateSelf();
      }
    });
    return ref.watch(settingsRepositoryProvider).get();
  },
);

/// Espace disque d'un répertoire (`null` = dossier de téléchargement
/// par défaut) — `{total, used, free}`.
final dirSpaceProvider = FutureProvider.autoDispose
    .family<Map<String, int>, String?>(
      (ref, dir) =>
          ref.watch(settingsRepositoryProvider).dirSpace(directory: dir),
    );

/// Espace disque d'une zone de stockage (`public`|`private`,
/// ADR-0018) — `{total, used, free}` de la racine de la zone.
final zoneSpaceProvider = FutureProvider.autoDispose
    .family<Map<String, int>, String>(
      (ref, area) =>
          ref.watch(settingsRepositoryProvider).dirSpace(area: area),
    );

/// État de la zone privée (`GET /api/private`) : `state`
/// (`locked`/`mounted`/`guest`), `downloads` du manifeste, compteurs
/// `orphans`. Rafraîchi à la main via `ref.invalidate`.
final privateZoneProvider =
    FutureProvider.autoDispose<Map<String, dynamic>>(
      (ref) => ref.watch(settingsRepositoryProvider).privateZone(),
    );

/// Items découverts par les flux RSS (`GET /api/rss`).
final rssItemsProvider = FutureProvider.autoDispose<List<Map<String, dynamic>>>(
  (ref) => ref.watch(settingsRepositoryProvider).rssItems(),
);

/// Versions connues du daemon (`GET /api/versioning/versions`).
final versionsProvider = FutureProvider.autoDispose<Map<String, dynamic>>(
  (ref) => ref.watch(settingsRepositoryProvider).versions(),
);

/// Bus d'enregistrement des sections à sauvegarde différée : chaque
/// section y pose `save`/`discard` ; la bannière « Enregistrer tout »
/// les invoque. Map mutable partagée (non réactive par nature).
final settingsSaveBusProvider =
    Provider<
      Map<String, ({Future<void> Function() save, void Function() discard})>
    >((ref) => {});

/// Sections ayant des modifications non enregistrées (puce « modifié »
/// + bannière « Enregistrer tout »).
final settingsDirtyProvider =
    NotifierProvider<SettingsDirtyNotifier, Set<String>>(
      SettingsDirtyNotifier.new,
    );

class SettingsDirtyNotifier extends Notifier<Set<String>> {
  @override
  Set<String> build() => const {};

  void add(String id) => state = state.contains(id) ? state : state.union({id});

  void remove(String id) => state = state.difference({id});

  void clear() => state = const {};
}
