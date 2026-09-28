/// Topics SSE émis par `tribler-api` — noms identiques aux
/// notifications `tribler.core.notifier.Notification` (Python), tels
/// qu'émis par `tribler-api/src/handlers/events.rs`.
abstract final class EventTopics {
  static const eventsStart = 'events_start';
  static const triblerShutdownStarted = 'tribler_shutdown_started';
  static const downloadStateChanged = 'download_state_changed';
  static const torrentFinished = 'torrent_finished';
  static const newTorrentMetadataCreated = 'new_torrent_metadata_created';
  static const torrentHealthUpdated = 'torrent_health_updated';

  /// Poussé par Python après une recherche distante. Le backend Rust
  /// intègre les réponses directement dans `channel_node` (sans
  /// événement dédié) — conservé pour compatibilité ascendante.
  static const remoteQueryResults = 'remote_query_results';
}
