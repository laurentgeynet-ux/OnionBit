import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../data/rest_diagnostic_repository.dart';
import '../../domain/diagnostic_models.dart';
import '../../domain/diagnostic_repository.dart';

final diagnosticRepositoryProvider = Provider<DiagnosticRepository>(
  (ref) => RestDiagnosticRepository(ref.watch(apiClientProvider)),
);

final overlaysProvider = FutureProvider.autoDispose<List<OverlayInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).overlays(),
);

final tunnelCircuitsProvider = FutureProvider.autoDispose<List<CircuitInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).circuits(),
);

final tunnelRelaysProvider = FutureProvider.autoDispose<List<RelayInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).relays(),
);

final tunnelExitsProvider = FutureProvider.autoDispose<List<ExitInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).exits(),
);

final tunnelSwarmsProvider = FutureProvider.autoDispose<List<SwarmInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).swarms(),
);

final tunnelPeersProvider = FutureProvider.autoDispose<List<TunnelPeerInfo>>(
  (ref) => ref.watch(diagnosticRepositoryProvider).tunnelPeers(),
);

final daemonLogsProvider = FutureProvider.autoDispose<String>(
  (ref) => ref.watch(diagnosticRepositoryProvider).logs(),
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
