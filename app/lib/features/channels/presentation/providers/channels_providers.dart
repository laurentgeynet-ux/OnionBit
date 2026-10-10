// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../../search/domain/torrent_result.dart';
import '../../data/rest_channels_repository.dart';
import '../../domain/channel.dart';
import '../../domain/channels_repository.dart';

final channelsRepositoryProvider = Provider<ChannelsRepository>(
  (ref) => RestChannelsRepository(ref.watch(apiClientProvider)),
);

/// Canaux suivis — `GET /api/channels`. La sync `channel_sync`
/// tourne côté daemon ; le contenu d'un canal se charge via
/// [channelContentsProvider].
final channelsProvider = AsyncNotifierProvider<ChannelsNotifier, List<Channel>>(
  ChannelsNotifier.new,
);

class ChannelsNotifier extends AsyncNotifier<List<Channel>> {
  @override
  Future<List<Channel>> build() => ref.watch(channelsRepositoryProvider).list();

  Future<void> refresh() async {
    state = await AsyncValue.guard(
      () => ref.read(channelsRepositoryProvider).list(),
    );
  }

  /// Abonnement à `(publicKey, id)` — la racine placeholder est
  /// créée côté daemon et la sync lance le remplissage.
  Future<void> subscribe(String publicKey, int id) async {
    await ref.read(channelsRepositoryProvider).subscribe(publicKey, id);
    await refresh();
  }

  Future<void> unsubscribe(Channel channel) async {
    await ref
        .read(channelsRepositoryProvider)
        .unsubscribe(channel.publicKey, channel.id);
    await refresh();
  }
}

/// Contenu persisté d'un canal — entrées `CHANNEL_TORRENT` au format
/// torrent (`num_seeders`/`num_leechers` joints comme dans
/// `/api/metadata`). Clé `(publicKey, id)` — un provider par canal.
final channelContentsProvider = FutureProvider.autoDispose
    .family<List<TorrentResult>, ({String publicKey, int id})>(
      (ref, key) =>
          ref.watch(channelsRepositoryProvider).contents(key.publicKey, key.id),
    );
