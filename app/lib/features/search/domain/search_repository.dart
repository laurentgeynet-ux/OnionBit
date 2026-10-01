// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'torrent_result.dart';

/// Requête distante acceptée par le daemon (`PUT /api/search/remote`).
class RemoteQuery {
  const RemoteQuery({required this.requestUuid, required this.peers});

  /// UUID de la requête — filtre les événements SSE
  /// `remote_query_results`.
  final String requestUuid;

  /// Pairs auxquels la requête a été diffusée (mids hex).
  final List<String> peers;
}

/// Contrat du dépôt recherche (`/api/metadata/*`, `/api/search/remote`).
abstract interface class SearchRepository {
  /// Torrents les plus populaires connus (contenu initial de la page
  /// recherche quand la requête est vide).
  Future<List<TorrentResult>> popular({int limit = 50});

  /// Recherche FTS locale dans `metadata.db`. `sortBy`/`sortDesc`
  /// suivent `sort_by`/`sort_desc` du backend (`HEALTH`, `name`,
  /// `size`, `date`…, `null` = tri de pertinence FTS).
  Future<List<TorrentResult>> searchLocal(
    String query, {
    String? sortBy,
    bool sortDesc = true,
  });

  /// Diffuse la requête aux pairs — réponses poussées via SSE.
  Future<RemoteQuery> searchRemote(String query);

  /// Sonde de santé à la demande (`GET
  /// /metadata/torrents/{ih}/health?refresh=1` — scrape immédiat des
  /// trackers connus). `null` = santé en cours de vérification côté
  /// daemon (`"checking"`).
  Future<({int seeders, int leechers})?> health(String infohash);
}
