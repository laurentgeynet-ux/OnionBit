// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../../../core/config/ui_log.dart';
import '../../search/data/rest_search_repository.dart';
import '../../search/domain/torrent_result.dart';
import '../domain/channel.dart';
import '../domain/channels_repository.dart';

/// Implémentation REST du dépôt canaux (`/api/channels*`).
class RestChannelsRepository implements ChannelsRepository {
  RestChannelsRepository(this._api);

  final ApiClient _api;

  @override
  Future<List<Channel>> list() async {
    final resp = await _api.get('/channels') as Map<String, dynamic>;
    return [
      for (final j
          in (resp['channels'] as List?)?.whereType<Map<String, dynamic>>() ??
              const <Map<String, dynamic>>[])
        Channel(
          publicKey: '${j['public_key'] ?? ''}',
          id: (j['id'] as num?)?.toInt() ?? 0,
          name: '${j['name'] ?? ''}',
          subscribed: j['subscribed'] == true,
          numEntries: (j['num_entries'] as num?)?.toInt() ?? 0,
        ),
    ];
  }

  @override
  Future<List<TorrentResult>> contents(String publicKey, int id) async =>
      RestSearchRepository.parseResults(
        await _api.get('/channels/$publicKey/$id'),
        TorrentSource.remote,
      );

  @override
  Future<void> subscribe(String publicKey, int id) async {
    uiLog('abonnement canal ${publicKey.substring(0, 8)}…/$id');
    await _api.put('/channels/$publicKey/$id/subscribe');
  }

  @override
  Future<void> unsubscribe(String publicKey, int id) async {
    uiLog('desabonnement canal ${publicKey.substring(0, 8)}…/$id');
    await _api.delete('/channels/$publicKey/$id/subscribe');
  }
}
