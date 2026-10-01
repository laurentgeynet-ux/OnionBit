// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Modèles du panneau Diagnostic — miroirs de `CircuitInfo`,
/// `RelayInfo`, `ExitInfo`, `SwarmInfo`, `TunnelPeerInfo`
/// (`onionbit-tunnel`) et de `overlays` (`onionbit-ipv8`).
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

/// Point d'introduction d'un swarm caché (`IntroductionPoint.to_dict`
/// pyipv8) — entrées des vues « Pairs DHT » et « Pairs PEX ».
class IntroPoint {
  const IntroPoint({
    required this.ip,
    required this.port,
    required this.publicKey,
    required this.seederPk,
    required this.source,
  });

  final String ip;
  final int port;
  final String publicKey;
  final String seederPk;

  /// Origine de la découverte (`dht`, `peer_discovery`/`pex`…).
  final String source;
}

/// Points d'introduction regroupés par info-hash de swarm
/// (`GET /api/ipv8/tunnel/peers/{dht,pex}` → `[{info_hash, peers}]`).
class SwarmPeers {
  const SwarmPeers({required this.infoHash, required this.peers});

  final String infoHash;
  final List<IntroPoint> peers;
}

/// Statistiques générales du daemon (`GET /api/statistics/tribler` →
/// `tribler_statistics`).
class OnionbitStats {
  const OnionbitStats({
    required this.dbSize,
    required this.numTorrents,
    required this.numChannels,
    required this.peers,
    required this.sessions,
    required this.version,
  });

  final int dbSize;
  final int numTorrents;
  final int numChannels;

  /// Pairs découverts par la stack IPv8 (`-1` = stack inactive).
  final int peers;

  /// Nombre de sessions moteur (principale + lanes anonymes ; `-1`
  /// = stack inactive).
  final int sessions;
  final String version;
}

/// Échantillon de débit d'un speed test de circuit (MiB/s,
/// `speed: {"up", "down"}` pyipv8).
class SpeedSample {
  const SpeedSample({required this.up, required this.down});

  final double up;
  final double down;
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
