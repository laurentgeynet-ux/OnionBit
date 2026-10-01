// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'diagnostic_models.dart';

/// Contrat du dépôt diagnostic (`/api/ipv8/*`, `/api/logging`).
abstract interface class DiagnosticRepository {
  Future<List<OverlayInfo>> overlays();
  Future<List<CircuitInfo>> circuits();
  Future<List<RelayInfo>> relays();
  Future<List<ExitInfo>> exits();
  Future<List<SwarmInfo>> swarms();
  Future<List<TunnelPeerInfo>> tunnelPeers();

  /// Points d'introduction stockés en DHT, groupés par info-hash
  /// (`GET /api/ipv8/tunnel/peers/dht`).
  Future<List<SwarmPeers>> dhtPeers();

  /// Points d'introduction du store PEX (`GET /api/ipv8/tunnel/peers/pex`).
  Future<List<SwarmPeers>> pexPeers();

  /// Statistiques générales (`GET /api/statistics/tribler`).
  Future<OnionbitStats> onionbitStats();

  /// Test de vitesse sur un circuit existant (`READY` + flag
  /// `PEER_FLAG_SPEED_TEST`) — flux `speed:` pyipv8 en MiB/s.
  Stream<SpeedSample> speedTestCircuit(int circuitId, {int testTimeMs = 5000});

  /// Test de vitesse sur un circuit `SPEED_TEST` temporaire créé pour
  /// l'occasion (détruit après le test).
  Stream<SpeedSample> speedTestNewCircuit(int hops, {int testTimeMs = 5000});

  /// Journal du daemon — réponse texte brut (`/api/logging`).
  Future<String> logs({int maxLines = 200});

  /// Mode debug du journal (`GET /api/ipv8/asyncio/debug` →
  /// `enable`) : à `true`, le daemon journalise les événements de
  /// cellules (create/extend/created/destroy, e2e, sorties).
  Future<bool> debugEnabled();

  /// Bascule le mode debug (`PUT /api/ipv8/asyncio/debug`).
  Future<void> setDebug(bool enable);
}
