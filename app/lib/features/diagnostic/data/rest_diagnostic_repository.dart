// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';

import '../../../core/api/api_client.dart';
import '../domain/diagnostic_models.dart';
import '../domain/diagnostic_repository.dart';

/// Implémentation REST du dépôt diagnostic.
class RestDiagnosticRepository implements DiagnosticRepository {
  RestDiagnosticRepository(this._api);

  final ApiClient _api;

  static List<Map<String, dynamic>> _list(dynamic resp, String key) =>
      ((resp as Map<String, dynamic>)[key] as List?)
          ?.whereType<Map<String, dynamic>>()
          .toList() ??
      const [];

  @override
  Future<List<OverlayInfo>> overlays() async => [
    for (final o in _list(await _api.get('/ipv8/overlays'), 'overlays'))
      OverlayInfo(
        // `overlay_name`/`peers` : clés réelles de `get_overlays`
        // pyipv8 — `peers` est la LISTE des pairs ({ip, port,
        // public_key}), pas un compteur.
        name: '${o['overlay_name'] ?? ''}',
        id: '${o['id'] ?? ''}',
        peers: (o['peers'] as List?)?.length ?? 0,
      ),
  ];

  @override
  Future<List<CircuitInfo>> circuits() async => [
    for (final c in _list(await _api.get('/ipv8/tunnel/circuits'), 'circuits'))
      CircuitInfo(
        id: (c['circuit_id'] as num?)?.toInt() ?? 0,
        goalHops: (c['goal_hops'] as num?)?.toInt() ?? 0,
        actualHops: (c['actual_hops'] as num?)?.toInt() ?? 0,
        type: '${c['type'] ?? ''}',
        state: '${c['state'] ?? ''}',
        bytesUp: (c['bytes_up'] as num?)?.toInt() ?? 0,
        bytesDown: (c['bytes_down'] as num?)?.toInt() ?? 0,
        exitFlags: (c['exit_flags'] as num?)?.toInt() ?? 0,
        infoHash: c['info_hash'] as String?,
        verifiedHops: [
          for (final h in (c['verified_hops'] as List?) ?? const []) '$h',
        ],
        unverifiedHop: '${c['unverified_hop'] ?? ''}',
        creationTime: (c['creation_time'] as num?)?.toInt() ?? 0,
      ),
  ];

  @override
  Future<List<RelayInfo>> relays() async => [
    for (final r in _list(await _api.get('/ipv8/tunnel/relays'), 'relays'))
      RelayInfo(
        circuitIn: (r['circuit_id_in'] as num?)?.toInt() ?? 0,
        circuitOut: (r['circuit_id_out'] as num?)?.toInt() ?? 0,
        rendezvous: r['rendezvous_relay'] == true,
        bytesUp: (r['bytes_up'] as num?)?.toInt() ?? 0,
        bytesDown: (r['bytes_down'] as num?)?.toInt() ?? 0,
      ),
  ];

  @override
  Future<List<ExitInfo>> exits() async => [
    for (final e in _list(await _api.get('/ipv8/tunnel/exits'), 'exits'))
      ExitInfo(
        circuitId: (e['circuit_id'] as num?)?.toInt() ?? 0,
        enabled: e['enabled'] == true,
      ),
  ];

  @override
  Future<List<SwarmInfo>> swarms() async => [
    for (final s in _list(await _api.get('/ipv8/tunnel/swarms'), 'swarms'))
      SwarmInfo(
        infoHash: '${s['info_hash'] ?? ''}',
        connections: (s['num_connections'] as num?)?.toInt() ?? 0,
        seeder: s['seeder'] == true,
      ),
  ];

  @override
  Future<List<TunnelPeerInfo>> tunnelPeers() async => [
    for (final p in _list(await _api.get('/ipv8/tunnel/peers'), 'peers'))
      TunnelPeerInfo(
        ip: '${p['ip'] ?? ''}',
        port: (p['port'] as num?)?.toInt() ?? 0,
        mid: '${p['mid'] ?? ''}',
        isKeyCompatible: p['is_key_compatible'] == true,
        flags: [
          for (final f in (p['flags'] as List?) ?? const [])
            (f as num?)?.toInt() ?? 0,
        ],
      ),
  ];

  IntroPoint _introPoint(Map<String, dynamic> p) {
    final addr = p['address'] as Map<String, dynamic>? ?? const {};
    return IntroPoint(
      ip: '${addr['ip'] ?? ''}',
      port: (addr['port'] as num?)?.toInt() ?? 0,
      publicKey: '${addr['public_key'] ?? ''}',
      seederPk: '${p['seeder_pk'] ?? ''}',
      source: '${p['source'] ?? ''}',
    );
  }

  /// Parse `[{info_hash, peers: [IntroductionPoint…]}]` — réponse
  /// tableau brut (sans enveloppe objet, format pyipv8).
  List<SwarmPeers> _swarmPeers(dynamic resp) => [
    for (final g in (resp as List?) ?? const [])
      if (g is Map<String, dynamic>)
        SwarmPeers(
          infoHash: '${g['info_hash'] ?? ''}',
          peers: [
            for (final p in (g['peers'] as List?) ?? const [])
              if (p is Map<String, dynamic>) _introPoint(p),
          ],
        ),
  ];

  @override
  Future<List<SwarmPeers>> dhtPeers() async =>
      _swarmPeers(await _api.get('/ipv8/tunnel/peers/dht'));

  @override
  Future<List<SwarmPeers>> pexPeers() async =>
      _swarmPeers(await _api.get('/ipv8/tunnel/peers/pex'));

  @override
  Future<OnionbitStats> onionbitStats() async {
    final resp = await _api.get('/statistics/tribler') as Map<String, dynamic>;
    final s = resp['tribler_statistics'] as Map<String, dynamic>? ?? const {};
    final lt = s['libtorrent'] as Map<String, dynamic>?;
    return OnionbitStats(
      dbSize: (s['db_size'] as num?)?.toInt() ?? 0,
      numTorrents: (s['num_torrents'] as num?)?.toInt() ?? 0,
      peers: (s['peers'] as num?)?.toInt() ?? -1,
      sessions: (lt?['sessions'] as List?)?.length ?? -1,
      version: '${s['endpoint_version'] ?? ''}',
      uptimeSec: (s['uptime_sec'] as num?)?.toInt() ?? -1,
      totalRecvBytes: (lt?['total_recv_bytes'] as num?)?.toInt() ?? -1,
      totalSentBytes: (lt?['total_sent_bytes'] as num?)?.toInt() ?? -1,
      laneHops: [
        for (final lane in (s['socks5_sessions'] as List?) ?? const [])
          if ((lane as Map)['hops'] case final num h) h.toInt(),
      ],
    );
  }

  @override
  Future<Ipv8Traffic> ipv8Traffic() async {
    final resp = await _api.get('/statistics/ipv8') as Map<String, dynamic>;
    final s = resp['ipv8_statistics'] as Map<String, dynamic>? ?? const {};
    final bw = s['bandwidth'] as Map<String, dynamic>?;
    return Ipv8Traffic(
      up: (s['total_up'] as num?)?.toInt() ?? 0,
      down: (s['total_down'] as num?)?.toInt() ?? 0,
      rateUp: (s['rate_up'] as num?)?.toInt() ?? 0,
      rateDown: (s['rate_down'] as num?)?.toInt() ?? 0,
      bandwidth: bw == null
          ? null
          : RelayBandwidth(
              effectiveRelayBps:
                  (bw['effective_relay_bps'] as num?)?.toInt() ?? 0,
              baseRttMs: (bw['base_rtt_ms'] as num?)?.toDouble(),
              minRttMs: (bw['min_rtt_ms'] as num?)?.toDouble(),
              rttSamples: (bw['rtt_samples'] as num?)?.toInt() ?? 0,
              relayMode: (bw['relay_mode'] as String?) ?? 'auto',
              relayDropped: (bw['relay_dropped'] as num?)?.toInt() ?? 0,
              servedBps: (bw['relay_served_bps'] as num?)?.toInt() ?? 0,
              servedBytes: (bw['relay_served_bytes'] as num?)?.toInt() ?? 0,
            ),
    );
  }

  @override
  Future<ConnectionsReport> connectionsReport() async {
    final resp = await _api.get('/connections') as Map<String, dynamic>;
    return ConnectionsReport(
      connections: [
        for (final c in _list(resp, 'connections'))
          ConnectionInfo(
            ip: '${c['ip'] ?? ''}',
            port: (c['port'] as num?)?.toInt() ?? 0,
            transports: [
              for (final t in (c['transports'] as List?) ?? const []) '$t',
            ],
            ipv8: c['ipv8'] == true,
            mid: '${c['mid'] ?? ''}',
            overlays: [
              for (final o in (c['overlays'] as List?) ?? const []) '$o',
            ],
            dht: c['dht'] == true,
            tunnelFlags: [
              for (final f in (c['tunnel_flags'] as List?) ?? const [])
                (f as num?)?.toInt() ?? 0,
            ],
            exitCircuits: [
              for (final cid in (c['exit_circuits'] as List?) ?? const [])
                (cid as num?)?.toInt() ?? 0,
            ],
            bittorrent: [
              for (final b in (c['bittorrent'] as List?) ?? const [])
                if (b is Map<String, dynamic>)
                  BtPeerConn(
                    infohash: '${b['infohash'] ?? ''}',
                    connKind: '${b['conn_kind'] ?? ''}',
                    state: '${b['state'] ?? ''}',
                    incoming: b['incoming'] == true,
                    client: '${b['client'] ?? ''}',
                    bytesUp: (b['bytes_up'] as num?)?.toInt() ?? 0,
                    bytesDown: (b['bytes_down'] as num?)?.toInt() ?? 0,
                  ),
            ],
          ),
      ],
      listeners: [
        for (final l in _list(resp, 'listeners'))
          ListenerInfo(
            protocol: '${l['protocol'] ?? ''}',
            address: '${l['address'] ?? ''}',
            circuitId: (l['circuit_id'] as num?)?.toInt(),
            hops: (l['hops'] as num?)?.toInt(),
          ),
      ],
    );
  }

  /// Parse le flux `speed: {"up":…,"down":…}` (MiB/s) du speed test
  /// pyipv8 — une ligne par échantillon ; les lignes illisibles sont
  /// ignorées.
  Stream<SpeedSample> _speedSamples(Stream<String> lines) =>
      lines.expand((l) sync* {
        if (!l.startsWith('speed:')) return;
        Object? j;
        try {
          j = jsonDecode(l.substring(6).trim());
        } catch (_) {
          return;
        }
        if (j is Map<String, dynamic>) {
          yield SpeedSample(
            up: (j['up'] as num?)?.toDouble() ?? 0,
            down: (j['down'] as num?)?.toDouble() ?? 0,
          );
        }
      });

  @override
  Stream<SpeedSample> speedTestCircuit(
    int circuitId, {
    int testTimeMs = 5000,
  }) => _speedSamples(
    _api.getStreamedLines(
      '/ipv8/tunnel/circuits/$circuitId/test',
      query: {'test_time_ms': '$testTimeMs'},
    ),
  );

  @override
  Stream<SpeedSample> speedTestNewCircuit(int hops, {int testTimeMs = 5000}) =>
      _speedSamples(
        _api.getStreamedLines(
          '/ipv8/tunnel/circuits/test',
          query: {'goal_hops': '$hops', 'test_time_ms': '$testTimeMs'},
        ),
      );

  @override
  Future<String> logs({int maxLines = 200}) =>
      _api.getText('/logging', query: {'max_lines': '$maxLines'});

  @override
  Future<bool> debugEnabled() async {
    final resp = await _api.get('/ipv8/asyncio/debug') as Map<String, dynamic>;
    return resp['enable'] == true;
  }

  @override
  Future<void> setDebug(bool enable) =>
      _api.put('/ipv8/asyncio/debug', body: {'enable': enable});
}
