// This file is part of OnionBit.
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

/// Réponse d'une recherche externe `.onion` (`GET
/// /api/search/external`, ADR-0028 §5) — synchrone contrairement à
/// [RemoteQuery] : les résultats sont dans la réponse et rediffusés
/// sur SSE `remote_query_results` (uuid partagé).
class ExternalQuery {
  const ExternalQuery({required this.uuid, required this.results});

  /// UUID de la requête — les notifications SSE portent le même.
  final String uuid;

  /// Résultats normalisés (déjà dédupliqués/scorés côté backend).
  final List<TorrentResult> results;
}

/// Contrat du dépôt recherche (`/api/metadata/*`, `/api/search/remote`).
abstract interface class SearchRepository {
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

  /// Recherche sur les backends `.onion` configurés via Tor
  /// (`source` = `tpb`, …). `ApiException.code` porte la cause
  /// (`tor_disabled`, `tor_bootstrapping`, `captcha_detected`…).
  Future<ExternalQuery> searchExternal(String query, String source);

  /// Sonde de santé à la demande (`GET
  /// /metadata/torrents/{ih}/health?refresh=1` — scrape immédiat des
  /// trackers connus). `null` = santé en cours de vérification côté
  /// daemon (`"checking"`).
  Future<({int seeders, int leechers})?> health(String infohash);
}
