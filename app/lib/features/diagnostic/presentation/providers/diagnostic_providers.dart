// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/config/ui_log.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_diagnostic_repository.dart';
import '../../domain/diagnostic_models.dart';
import '../../domain/diagnostic_repository.dart';

/// Intervalle de sondage des sondes diagnostic — comme le poll 2 s
/// de `GET /api/downloads` (la GUI Tribler rafraîchit ces vues en
/// continu).
const _kDiagnosticPoll = Duration(seconds: 2);

final diagnosticRepositoryProvider = Provider<DiagnosticRepository>(
  (ref) => RestDiagnosticRepository(ref.watch(apiClientProvider)),
);

final overlaysProvider = FutureProvider.autoDispose<List<OverlayInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).overlays();
  },
);

final tunnelCircuitsProvider = FutureProvider.autoDispose<List<CircuitInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).circuits();
  },
);

final tunnelRelaysProvider = FutureProvider.autoDispose<List<RelayInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).relays();
  },
);

final tunnelExitsProvider = FutureProvider.autoDispose<List<ExitInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).exits();
  },
);

final tunnelSwarmsProvider = FutureProvider.autoDispose<List<SwarmInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).swarms();
  },
);

final tunnelPeersProvider = FutureProvider.autoDispose<List<TunnelPeerInfo>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).tunnelPeers();
  },
);

/// Points d'introduction stockés en DHT (swarms cachés).
final dhtPeersProvider = FutureProvider.autoDispose<List<SwarmPeers>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).dhtPeers();
  },
);

/// Points d'introduction du store PEX.
final pexPeersProvider = FutureProvider.autoDispose<List<SwarmPeers>>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).pexPeers();
  },
);

/// Statistiques générales du daemon (`/api/statistics/tribler`).
final onionbitStatsProvider = FutureProvider.autoDispose<OnionbitStats>(
  (ref) {
    ref.watch(tickProvider(const Duration(seconds: 5)));
    return ref.watch(diagnosticRepositoryProvider).onionbitStats();
  },
);

/// Compteurs d'octets de l'endpoint overlay
/// (`/api/statistics/ipv8` — `total_up`/`total_down`).
final ipv8TrafficProvider = FutureProvider.autoDispose<Ipv8Traffic>(
  (ref) {
    ref.watch(tickProvider(const Duration(seconds: 5)));
    return ref.watch(diagnosticRepositoryProvider).ipv8Traffic();
  },
);

/// État de la carte « Trafic tunnel » : débit instantané (o/s)
/// + cumuls d'octets de l'endpoint overlay depuis le démarrage
/// du daemon.
class TunnelTraffic {
  const TunnelTraffic({
    this.rateDown,
    this.rateUp,
    this.totalDown = 0,
    this.totalUp = 0,
  });

  /// Débit descendant instantané — `null` tant que deux
  /// échantillons de compteurs n'ont pas été observés.
  final int? rateDown;

  /// Débit montant instantané (idem).
  final int? rateUp;

  /// Octets cumulés reçus/émis par l'endpoint overlay
  /// (`total_down`/`total_up` bruts).
  final int totalDown;
  final int totalUp;

  bool get hasRate => rateDown != null && rateUp != null;
}

/// « Trafic tunnel » — débit calculé par différence des compteurs
/// `total_up`/`total_down` entre deux émissions successives de
/// `ipv8TrafficProvider` (déjà sondé toutes les 5 s : une seule
/// requête `/api/statistics/ipv8` est partagée par cette carte,
/// l'onglet Statistiques et la barre d'état). Aucune dépendance
/// au dépôt ni à `apiClientProvider` : le notifier n'est pas
/// reconstruit quand le watchdog SSE recrée le client HTTP.
final tunnelTrafficProvider =
    NotifierProvider.autoDispose<TunnelTrafficNotifier, TunnelTraffic>(
      TunnelTrafficNotifier.new,
    );

class TunnelTrafficNotifier extends Notifier<TunnelTraffic> {
  ({int down, int up, DateTime at})? _sample;

  @override
  TunnelTraffic build() {
    ref.listen(ipv8TrafficProvider, (_, next) {
      // Seules les données fraîches comptent : l'`AsyncLoading` de
      // rafraîchissement re-porte l'ancienne valeur et corromprait
      // la fenêtre de mesure.
      if (next case AsyncData(:final value)) _ingest(value);
    });
    // Les compteurs sont souvent déjà sondés (barre d'état) :
    // baseline immédiate → débit dès le prochain tick.
    final cached = ref.read(ipv8TrafficProvider).value;
    if (cached != null) {
      _sample = (down: cached.down, up: cached.up, at: DateTime.now());
      return TunnelTraffic(totalDown: cached.down, totalUp: cached.up);
    }
    return const TunnelTraffic();
  }

  void _ingest(Ipv8Traffic s) {
    final now = DateTime.now();
    final prev = _sample;
    _sample = (down: s.down, up: s.up, at: now);
    if (prev == null) {
      state = TunnelTraffic(totalDown: s.down, totalUp: s.up);
      return;
    }
    final secs = now.difference(prev.at).inMicroseconds / 1e6;
    if (secs <= 0) return;
    // Compteurs remis à zéro au redémarrage du daemon : un delta
    // négatif est ramené à 0 plutôt qu'affiché.
    state = TunnelTraffic(
      rateDown: ((s.down - prev.down) / secs).round().clamp(0, 1 << 62),
      rateUp: ((s.up - prev.up) / secs).round().clamp(0, 1 << 62),
      totalDown: s.down,
      totalUp: s.up,
    );
  }
}

final daemonLogsProvider = FutureProvider.autoDispose<String>(
  (ref) {
    ref.watch(tickProvider(_kDiagnosticPoll));
    return ref.watch(diagnosticRepositoryProvider).logs();
  },
);

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
