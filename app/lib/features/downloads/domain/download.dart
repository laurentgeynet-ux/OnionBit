import 'download_peer.dart';
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
    required this.queuePosition,
    required this.autoManaged,
    required this.userStopped,
    required this.uploadLimit,
    required this.downloadLimit,
    required this.seedingRatio,
    required this.timeAdded,
    required this.timeFinished,
    required this.trackers,
    required this.peers,
    this.isPrivate = false,
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

  /// Position dans la file (`queue_position` ; `-1` = non géré).
  final int queuePosition;

  /// File d'attente automatique (`auto_managed`) — le gestionnaire de
  /// file pause/reprend selon `active_*`.
  final bool autoManaged;

  /// Arrêt demandé par l'utilisateur (distinct de la pause de file).
  final bool userStopped;

  /// Limite d'upload octets/s (0 = illimité).
  final int uploadLimit;

  /// Limite de download octets/s (0 = illimité).
  final int downloadLimit;

  /// Ratio de seed individuel (0 = pas de borne individuelle).
  final double seedingRatio;

  /// Timestamps epoch secondes (0 = inconnu).
  final int timeAdded;
  final int timeFinished;

  final List<DownloadTracker> trackers;

  /// Pairs connectés (`?get_peers=1`) — vide si la liste n'a pas été
  /// demandée.
  final List<DownloadPeer> peers;

  /// Flag `private` du metainfo (extension backend) — trackers
  /// uniquement, DHT/PEX désactivés. Permet d'offrir « Republier en
  /// anonyme » (jumeau public).
  final bool isPrivate;

  bool get isActive => status == 'DOWNLOADING' || status == 'SEEDING';
  bool get isPaused => status == 'STOPPED';
  bool get isError => status == 'STOPPED_ON_ERROR' || error.isNotEmpty;

  /// En file d'attente (`QUEUED` Python = code 11) ou pause de file.
  bool get isQueued => statusCode == 11;
}
