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
        name: '${o['name'] ?? ''}',
        id: '${o['id'] ?? ''}',
        peers: (o['peers'] as num?)?.toInt() ?? 0,
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
        publicKey: '${p['public_key'] ?? ''}',
        flags: (p['flags'] as num?)?.toInt() ?? 0,
      ),
  ];

  @override
  Future<String> logs({int maxLines = 200}) =>
      _api.getText('/logging', query: {'max_lines': '$maxLines'});
}
