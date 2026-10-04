// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Topics SSE émis par `onionbit-api` — noms identiques aux
/// notifications `tribler.core.notifier.Notification` (Python), tels
/// qu'émis par `onionbit-api/src/handlers/events.rs`.
abstract final class EventTopics {
  static const eventsStart = 'events_start';
  static const triblerShutdownStarted = 'tribler_shutdown_started';
  static const downloadStateChanged = 'download_state_changed';
  static const torrentStatusChanged = 'torrent_status_changed';
  static const torrentFinished = 'torrent_finished';
  static const newTorrentMetadataCreated = 'new_torrent_metadata_created';
  static const torrentHealthUpdated = 'torrent_health_updated';

  /// Poussé par Python après une recherche distante. Le backend Rust
  /// intègre les réponses directement dans `channel_node` (sans
  /// événement dédié) — conservé pour compatibilité ascendante.
  static const remoteQueryResults = 'remote_query_results';

  /// Poussé par `POST /api/settings` — les clients doivent recharger
  /// l'arbre de configuration (boucle multi-clients).
  static const settingsChanged = 'settings_changed';

  /// Un download sur lane anonyme s'est révélé `private=1` après
  /// résolution du metainfo (le flag n'est pas dans le magnet).
  static const privateTorrentDetected = 'private_torrent_detected';
}
