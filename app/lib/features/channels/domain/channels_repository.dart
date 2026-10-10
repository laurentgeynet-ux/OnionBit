// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../search/domain/torrent_result.dart';
import 'channel.dart';

/// Contrat du dépôt canaux (`/api/channels*` — ADR-0025).
abstract interface class ChannelsRepository {
  /// Canaux suivis (`GET /api/channels` — abonnements uniquement).
  Future<List<Channel>> list();

  /// Contenu persisté du canal `(publicKey, id)` (`GET
  /// /api/channels/{pk}/{id}` — entrées `CHANNEL_TORRENT` au format
  /// torrent de `/api/metadata`, tri timestamp desc).
  Future<List<TorrentResult>> contents(String publicKey, int id);

  /// Suit le canal `(publicKey, id)` — crée la racine placeholder
  /// côté daemon si inconnue, `channel_sync` la tire des pairs.
  Future<void> subscribe(String publicKey, int id);

  /// Se désabonne — les entrées persistées restent, la sync
  /// s'arrête au prochain tick.
  Future<void> unsubscribe(String publicKey, int id);
}
