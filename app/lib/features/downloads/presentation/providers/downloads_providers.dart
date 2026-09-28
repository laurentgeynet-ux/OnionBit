import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_downloads_repository.dart';
import '../../domain/download.dart';
import '../../domain/download_file.dart';
import '../../domain/downloads_repository.dart';

/// Intervalle de rafraîchissement de la liste (comme la GUI Tribler,
/// qui re-poll `GET /api/downloads` en continu).
const _kPollInterval = Duration(seconds: 2);

final downloadsRepositoryProvider = Provider<DownloadsRepository>(
  (ref) => RestDownloadsRepository(ref.watch(apiClientProvider)),
);

/// Liste des téléchargements — poll périodique + rafraîchissement
/// immédiat sur les événements SSE pertinents.
final downloadsProvider =
    AsyncNotifierProvider<DownloadsNotifier, List<Download>>(
      DownloadsNotifier.new,
    );

class DownloadsNotifier extends AsyncNotifier<List<Download>> {
  Timer? _timer;

  @override
  Future<List<Download>> build() async {
    final repo = ref.watch(downloadsRepositoryProvider);
    _timer = Timer.periodic(_kPollInterval, (_) => _refresh());
    ref.onDispose(() => _timer?.cancel());
    ref.listen(daemonEventsProvider, (_, next) {
      final topic = next.value?.topic;
      if (topic == EventTopics.downloadStateChanged ||
          topic == EventTopics.torrentFinished) {
        _refresh();
      }
    });
    return repo.list();
  }

  Future<void> _refresh() async {
    if (!ref.mounted) return;
    state = await AsyncValue.guard(ref.read(downloadsRepositoryProvider).list);
  }

  Future<void> refresh() => _refresh();

  /// Exécute une action du dépôt puis rafraîchit — l'erreur remonte
  /// à l'appelant (snack bar).
  Future<void> _run(
    Future<void> Function(DownloadsRepository repo) action,
  ) async {
    await action(ref.read(downloadsRepositoryProvider));
    await _refresh();
  }

  Future<void> pause(String infohash) => _run((r) => r.pause(infohash));
  Future<void> resume(String infohash) => _run((r) => r.resume(infohash));
  Future<void> remove(String infohash, {bool deleteFiles = false}) =>
      _run((r) => r.remove(infohash, deleteFiles: deleteFiles));
  Future<void> setAnonHops(String infohash, int hops) =>
      _run((r) => r.setAnonHops(infohash, hops));
}

/// Sélection courante (infohashes) — multi-sélection par cases.
final downloadSelectionProvider =
    NotifierProvider<DownloadSelectionNotifier, Set<String>>(
      DownloadSelectionNotifier.new,
    );

class DownloadSelectionNotifier extends Notifier<Set<String>> {
  @override
  Set<String> build() => const {};

  void toggle(String infohash) => state = state.contains(infohash)
      ? state.difference({infohash})
      : state.union({infohash});

  void selectOnly(String infohash) => state = {infohash};

  void clear() => state = const {};
}

/// Débits agrégés de tous les téléchargements (barre d'état).
final totalSpeedsProvider = Provider<({int down, int up})>((ref) {
  final downloads = ref.watch(downloadsProvider).value ?? const [];
  var down = 0;
  var up = 0;
  for (final d in downloads) {
    down += d.speedDown;
    up += d.speedUp;
  }
  return (down: down, up: up);
});

/// Fichiers du téléchargement sélectionné (onglet « Fichiers »).
final downloadFilesProvider = FutureProvider.autoDispose
    .family<List<DownloadFile>, String>(
      (ref, infohash) => ref.watch(downloadsRepositoryProvider).files(infohash),
    );
