// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
    isPrivate: this['private'] == true,
    // ADR-0018 : zone de stockage `public`/`private` ; `locked_area`
    // = zone privée mais identité verrouillée (contenu inaccessible).
    storageArea: (this['storage_area'] as String?) ?? 'public',
    lockedArea: this['locked_area'] == true,
    safeSeeding: this['safe_seeding'] == true,
    uploaded: (this['all_time_upload'] as num?)?.toInt() ?? 0,
    downloaded: (this['all_time_download'] as num?)?.toInt() ?? 0,
    ratio: (this['all_time_ratio'] as num?)?.toDouble() ?? 0,
    sessionUploaded: (this['session_upload'] as num?)?.toInt() ?? 0,
    sessionDownloaded: (this['session_download'] as num?)?.toInt() ?? 0,
    error: (this['error'] as String?) ?? '',
    destination: (this['destination'] as String?) ?? '',
    streamable: this['streamable'] == true,
    queuePosition: (this['queue_position'] as num?)?.toInt() ?? -1,
    autoManaged: this['auto_managed'] == true,
    userStopped: this['user_stopped'] == true,
    uploadLimit: (this['upload_limit'] as num?)?.toInt() ?? 0,
    downloadLimit: (this['download_limit'] as num?)?.toInt() ?? 0,
    seedingRatio: (this['seeding_ratio'] as num?)?.toDouble() ?? 0,
    timeAdded: (this['time_added'] as num?)?.toInt() ?? 0,
    timeFinished: (this['time_finished'] as num?)?.toInt() ?? 0,
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
