// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/config/ui_log.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_diagnostic_repository.dart';
import '../../domain/diagnostic_models.dart';
import '../../domain/diagnostic_repository.dart';

/// Intervalle de sondage des sondes structurelles (overlays,
/// circuits, relais, sorties, pairs, journaux) — la cadence
/// « auto-refresh 5 s » affichée par l'onglet Vue d'ensemble. Ces
/// listes évoluent lentement : 2 s multipliait les requêtes par ~2,5
/// sans gain visible.
const _kDiagnosticPoll = Duration(seconds: 5);

/// Cadence plus vive pour les débits live (carte « Trafic tunnel »,
/// barre d'état) — le daemon maintient la fenêtre glissante, le
/// client ne fait que lire.
const _kTrafficPoll = Duration(seconds: 2);

final diagnosticRepositoryProvider = Provider<DiagnosticRepository>(
  (ref) => RestDiagnosticRepository(ref.watch(apiClientProvider)),
);

final overlaysProvider = FutureProvider.autoDispose<List<OverlayInfo>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).overlays();
});

final tunnelCircuitsProvider = FutureProvider.autoDispose<List<CircuitInfo>>((
  ref,
) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).circuits();
});

final tunnelRelaysProvider = FutureProvider.autoDispose<List<RelayInfo>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).relays();
});

final tunnelExitsProvider = FutureProvider.autoDispose<List<ExitInfo>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).exits();
});

final tunnelSwarmsProvider = FutureProvider.autoDispose<List<SwarmInfo>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).swarms();
});

final tunnelPeersProvider = FutureProvider.autoDispose<List<TunnelPeerInfo>>((
  ref,
) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).tunnelPeers();
});

/// Points d'introduction stockés en DHT (swarms cachés).
final dhtPeersProvider = FutureProvider.autoDispose<List<SwarmPeers>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).dhtPeers();
});

/// Points d'introduction du store PEX.
final pexPeersProvider = FutureProvider.autoDispose<List<SwarmPeers>>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).pexPeers();
});

/// Statistiques générales du daemon (`/api/statistics/tribler`).
final onionbitStatsProvider = FutureProvider.autoDispose<OnionbitStats>((ref) {
  ref.watch(tickProvider(const Duration(seconds: 5)));
  return ref.watch(diagnosticRepositoryProvider).onionbitStats();
});

/// Compteurs d'octets de l'endpoint overlay
/// (`/api/statistics/ipv8` — `total_up`/`total_down`) + débits
/// mesurés par le daemon (`rate_up`/`rate_down`, fenêtre glissante
/// côté Rust — plus rien à dériver côté client). Sondé à
/// `_kTrafficPoll` (2 s) : la carte « Trafic tunnel », l'onglet
/// Statistiques et la barre d'état partagent la requête.
final ipv8TrafficProvider = FutureProvider.autoDispose<Ipv8Traffic>((ref) {
  ref.watch(tickProvider(_kTrafficPoll));
  return ref.watch(diagnosticRepositoryProvider).ipv8Traffic();
});

/// Endpoints distants agrégés par `ip:port` + sockets d'écoute
/// locales (`GET /api/connections`, extension Rust) — onglet
/// « Connexions ».
final connectionsProvider = FutureProvider.autoDispose<ConnectionsReport>((
  ref,
) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).connectionsReport();
});

final daemonLogsProvider = FutureProvider.autoDispose<String>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return ref.watch(diagnosticRepositoryProvider).logs();
});

/// Journal UI (`logs/ui.log` — connexion daemon, bascules SSE,
/// ajouts de téléchargement, recherches distantes…).
final uiConnectLogProvider = FutureProvider.autoDispose<String>((ref) {
  ref.watch(tickProvider(_kDiagnosticPoll));
  return readUiLog();
});

/// État du mode debug du journal (`GET /api/ipv8/asyncio/debug` →
/// `enable`) — piloté par l'interrupteur de l'onglet Journaux.
final debugLogEnabledProvider = FutureProvider.autoDispose<bool>(
  (ref) => ref.watch(diagnosticRepositoryProvider).debugEnabled(),
);

/// État de la lane anonyme pour la barre d'état — sondée toutes les
/// 10 s. Une erreur API (stack IPv8 inactive) signifie « désactivée »,
/// pas « en panne ».
final anonLaneProvider = FutureProvider.autoDispose<AnonLaneStatus>((
  ref,
) async {
  ref.watch(tickProvider(const Duration(seconds: 10)));
  try {
    final circuits = await ref.watch(diagnosticRepositoryProvider).circuits();
    final ready = circuits.where((c) => c.ready).length;
    return AnonLaneStatus(
      state: ready > 0 ? AnonLaneState.ready : AnonLaneState.waiting,
      readyCircuits: ready,
      totalCircuits: circuits.length,
    );
  } catch (_) {
    return AnonLaneStatus.disabled;
  }
});
