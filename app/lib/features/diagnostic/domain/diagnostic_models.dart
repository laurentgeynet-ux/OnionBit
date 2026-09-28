/// Modèles du panneau Diagnostic — miroirs de `CircuitInfo`,
/// `RelayInfo`, `ExitInfo`, `SwarmInfo`, `TunnelPeerInfo`
/// (`tribler-tunnel`) et de `overlays` (`tribler-ipv8`).
class OverlayInfo {
  const OverlayInfo({
    required this.name,
    required this.id,
    required this.peers,
  });

  final String name;
  final String id;
  final int peers;
}

class CircuitInfo {
  const CircuitInfo({
    required this.id,
    required this.goalHops,
    required this.actualHops,
    required this.type,
    required this.state,
    required this.bytesUp,
    required this.bytesDown,
    required this.exitFlags,
    required this.infoHash,
    required this.verifiedHops,
    required this.unverifiedHop,
    required this.creationTime,
  });

  final int id;
  final int goalHops;
  final int actualHops;
  final String type;
  final String state;
  final int bytesUp;
  final int bytesDown;
  final int exitFlags;
  final String? infoHash;

  /// `verified_hops` : mid hex de chaque saut vérifié, dans l'ordre
  /// du circuit (la route réellement prise).
  final List<String> verifiedHops;

  /// `unverified_hop` : mid hex du saut en cours d'ajout (`''` sinon).
  final String unverifiedHop;

  /// `creation_time` (epoch secondes).
  final int creationTime;

  bool get ready => state == 'READY';
}

class RelayInfo {
  const RelayInfo({
    required this.circuitIn,
    required this.circuitOut,
    required this.rendezvous,
    required this.bytesUp,
    required this.bytesDown,
  });

  final int circuitIn;
  final int circuitOut;
  final bool rendezvous;
  final int bytesUp;
  final int bytesDown;
}

class ExitInfo {
  const ExitInfo({required this.circuitId, required this.enabled});

  final int circuitId;
  final bool enabled;
}

class SwarmInfo {
  const SwarmInfo({
    required this.infoHash,
    required this.connections,
    required this.seeder,
  });

  final String infoHash;
  final int connections;
  final bool seeder;
}

class TunnelPeerInfo {
  const TunnelPeerInfo({
    required this.ip,
    required this.port,
    required this.mid,
    required this.isKeyCompatible,
    required this.flags,
  });

  final String ip;
  final int port;
  final String mid;
  final bool isKeyCompatible;

  /// `PEER_FLAG_*` annoncés — liste d'entiers (set Python), pas un
  /// bitmask.
  final List<int> flags;
}

/// État de la lane anonyme pour la barre d'état — « honnête » :
/// prête = au moins un circuit `READY`, attente = circuits absents,
/// désactivée = stack IPv8/tunnel inactive (erreur API).
enum AnonLaneState { disabled, waiting, ready }

class AnonLaneStatus {
  const AnonLaneStatus({
    required this.state,
    required this.readyCircuits,
    required this.totalCircuits,
  });

  final AnonLaneState state;
  final int readyCircuits;
  final int totalCircuits;

  static const disabled = AnonLaneStatus(
    state: AnonLaneState.disabled,
    readyCircuits: 0,
    totalCircuits: 0,
  );
}
