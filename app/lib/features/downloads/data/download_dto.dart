import '../domain/download.dart';
import '../domain/download_peer.dart';
import '../domain/download_tracker.dart';

/// Extension de parsing du dict `info` (format Python, voir
/// `docs/reference_tribler/api_rest_mapping.md`).
extension DownloadJson on Map<String, dynamic> {
  Download toDownload() => Download(
    infohash: (this['infohash'] as String?) ?? '',
    name: (this['name'] as String?) ?? '',
    progress: (this['progress'] as num?)?.toDouble() ?? 0,
    size: (this['size'] as num?)?.toInt() ?? 0,
    speedDown: (this['speed_down'] as num?)?.toInt() ?? 0,
    speedUp: (this['speed_up'] as num?)?.toInt() ?? 0,
    status: (this['status'] as String?) ?? '',
    statusCode: (this['status_code'] as num?)?.toInt() ?? 0,
    // `eta` est un float de secondes depuis l'alignement du
    // contrat (plus une chaîne formatée).
    etaSeconds: (this['eta'] as num?)?.toDouble() ?? 0,
    numSeeds: (this['num_seeds'] as num?)?.toInt() ?? 0,
    numPeers: (this['num_peers'] as num?)?.toInt() ?? 0,
    numConnectedPeers: (this['num_connected_peers'] as num?)?.toInt() ?? 0,
    hops: (this['hops'] as num?)?.toInt() ?? 0,
    anonDownload: this['anon_download'] == true,
    safeSeeding: this['safe_seeding'] == true,
    uploaded: (this['all_time_upload'] as num?)?.toInt() ?? 0,
    downloaded: (this['all_time_download'] as num?)?.toInt() ?? 0,
    ratio: (this['all_time_ratio'] as num?)?.toDouble() ?? 0,
    error: (this['error'] as String?) ?? '',
    destination: (this['destination'] as String?) ?? '',
    streamable: this['streamable'] == true,
    trackers: [
      for (final t in (this['trackers'] as List?) ?? const [])
        if (t is Map<String, dynamic>)
          DownloadTracker(
            url: '${t['url'] ?? ''}',
            status: '${t['status'] ?? 'Not contacted yet'}',
            peers: (t['peers'] as num?)?.toInt() ?? -1,
            seeds: (t['seeds'] as num?)?.toInt() ?? -1,
            leeches: (t['leeches'] as num?)?.toInt() ?? -1,
          ),
    ],
    peers: [
      for (final p in (this['peers'] as List?) ?? const [])
        if (p is Map<String, dynamic>)
          DownloadPeer(
            ip: '${p['ip'] ?? ''}',
            port: (p['port'] as num?)?.toInt() ?? 0,
            extendedVersion: '${p['extended_version'] ?? ''}',
            direction: '${p['direction'] ?? ''}',
            downrate: (p['downrate'] as num?)?.toInt() ?? 0,
            uprate: (p['uprate'] as num?)?.toInt() ?? 0,
            dtotal: (p['dtotal'] as num?)?.toInt() ?? 0,
            utotal: (p['utotal'] as num?)?.toInt() ?? 0,
            connectionType: '${p['connection_type'] ?? ''}',
          ),
    ],
  );
}
