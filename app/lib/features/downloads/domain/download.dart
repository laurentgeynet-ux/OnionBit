import 'download_tracker.dart';

/// Entité métier `Download` — miroir de `DownloadInfo` (DTO de
/// `tribler-api`, lui-même miroir du dict `info` Python).
class Download {
  const Download({
    required this.infohash,
    required this.name,
    required this.progress,
    required this.size,
    required this.speedDown,
    required this.speedUp,
    required this.status,
    required this.statusCode,
    required this.etaSeconds,
    required this.numSeeds,
    required this.numPeers,
    required this.numConnectedPeers,
    required this.hops,
    required this.anonDownload,
    required this.safeSeeding,
    required this.uploaded,
    required this.downloaded,
    required this.ratio,
    required this.error,
    required this.destination,
    required this.streamable,
    required this.trackers,
  });

  final String infohash;
  final String name;
  final double progress;
  final int size;
  final int speedDown;
  final int speedUp;
  final String status;
  final int statusCode;

  /// ETA en secondes (float — `get_eta()` Python ; 0 = inconnu).
  final double etaSeconds;
  final int numSeeds;
  final int numPeers;
  final int numConnectedPeers;
  final int hops;
  final bool anonDownload;
  final bool safeSeeding;
  final int uploaded;
  final int downloaded;
  final double ratio;
  final String error;
  final String destination;
  final bool streamable;
  final List<DownloadTracker> trackers;

  bool get isActive => status == 'DOWNLOADING' || status == 'SEEDING';
  bool get isPaused => status == 'STOPPED';
  bool get isError => status == 'STOPPED_ON_ERROR' || error.isNotEmpty;
}
