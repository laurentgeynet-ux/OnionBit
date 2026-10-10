// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../data/rest_private_zone_repository.dart';
import '../../domain/private_zone.dart';

final privateZoneRepositoryProvider = Provider<RestPrivateZoneRepository>(
  (ref) => RestPrivateZoneRepository(ref.watch(apiClientProvider)),
);

/// Catalogue typé de la zone privée (`GET /api/private`) — la
/// visibilité sidebar reste sur `privateZoneProvider` des réglages
/// (`state == 'mounted'`), ce provider sert l'explorateur.
final privateZoneCatalogProvider =
    AsyncNotifierProvider<PrivateZoneNotifier, PrivateZoneCatalog>(
      PrivateZoneNotifier.new,
    );

class PrivateZoneNotifier extends AsyncNotifier<PrivateZoneCatalog> {
  @override
  Future<PrivateZoneCatalog> build() =>
      ref.watch(privateZoneRepositoryProvider).catalog();

  Future<void> refresh() async {
    state = await AsyncValue.guard(
      () => ref.read(privateZoneRepositoryProvider).catalog(),
    );
  }
}

/// Fichiers d'une entrée du manifeste — chargés à l'expansion de la
/// tuile (clé = infohash réel ou `row_key`, indifférent pour l'API).
final privateFilesProvider = FutureProvider.autoDispose
    .family<List<PrivateFileEntry>, String>(
      (ref, key) => ref.watch(privateZoneRepositoryProvider).files(key),
    );
