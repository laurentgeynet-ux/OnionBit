import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_downloads_repository.dart';
import '../../domain/download.dart';
import '../../domain/download_file.dart';
import '../../domain/download_tracker.dart';
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
  Future<void> moveInQueue(String infohash, QueueOp op) =>
      _run((r) => r.moveInQueue(infohash, op));
  Future<void> setAutoManaged(String infohash, bool enabled) =>
      _run((r) => r.setAutoManaged(infohash, enabled));
  Future<void> setRateLimits(
    String infohash, {
    int? uploadLimit,
    int? downloadLimit,
  }) => _run(
    (r) => r.setRateLimits(
      infohash,
      uploadLimit: uploadLimit,
      downloadLimit: downloadLimit,
    ),
  );
  Future<void> setSeedingRatio(String infohash, double ratio) =>
      _run((r) => r.setSeedingRatio(infohash, ratio));
  Future<void> resetSeedingRatio(String infohash) =>
      _run((r) => r.resetSeedingRatio(infohash));
  Future<String> clonePublic(String infohash, {int? anonHops}) async {
    final ih = await ref
        .read(downloadsRepositoryProvider)
        .clonePublic(infohash, anonHops: anonHops);
    await _refresh();
    return ih;
  }

  Future<void> recheck(String infohash) => _run((r) => r.recheck(infohash));
  Future<void> moveStorage(
    String infohash, {
    required String destination,
    String? completedDir,
  }) => _run(
    (r) =>
        r.moveStorage(infohash, destination: destination, completedDir: completedDir),
  );
  Future<void> setFilePriority(String infohash, int index, int priority) =>
      _run((r) => r.setFilePriority(infohash, index, priority));

  Future<void> setFileIncluded(
    String infohash,
    int index,
    bool included,
    List<DownloadFile> files,
  ) async {
    // `selected_files` = liste des indices inclus ; le PATCH remplace
    // la sélection entière (liste vide = tous inclus).
    final selected = files
        .where((f) => f.index == index ? included : f.included)
        .map((f) => f.index)
        .toList();
    final repo = ref.read(downloadsRepositoryProvider);
    await repo.setSelectedFiles(infohash, selected);
    ref.invalidate(downloadFilesProvider(infohash));
    await _refresh();
  }

  Future<void> addTracker(String infohash, String url) async {
    await ref.read(downloadsRepositoryProvider).addTracker(infohash, url);
    ref.invalidate(downloadTrackersProvider(infohash));
  }

  Future<void> removeTracker(String infohash, String url) async {
    await ref.read(downloadsRepositoryProvider).removeTracker(infohash, url);
    ref.invalidate(downloadTrackersProvider(infohash));
  }

  Future<void> addDefaultTrackers(String infohash) async {
    await ref.read(downloadsRepositoryProvider).addDefaultTrackers(infohash);
    ref.invalidate(downloadTrackersProvider(infohash));
  }

  Future<void> forceTrackerAnnounce(String infohash, String url) async {
    await ref
        .read(downloadsRepositoryProvider)
        .forceTrackerAnnounce(infohash, url);
    ref.invalidate(downloadTrackersProvider(infohash));
  }
}

/// Colonnes triables de la table desktop.
enum DownloadSort {
  name,
  size,
  progress,
  status,
  down,
  up,
  eta,
  peers,
  ratio,
  added,
}

/// Tri courant de la table : colonne + sens (ascendant par défaut).
final downloadSortProvider =
    NotifierProvider<DownloadSortNotifier, ({DownloadSort col, bool asc})>(
      DownloadSortNotifier.new,
    );

class DownloadSortNotifier extends Notifier<({DownloadSort col, bool asc})> {
  @override
  ({DownloadSort col, bool asc}) build() =>
      (col: DownloadSort.name, asc: true);

  /// Clique sur un en-tête : même colonne = inverse le sens, sinon
  /// nouvelle colonne en ascendant.
  void tap(DownloadSort col) => state = state.col == col
      ? (col: col, asc: !state.asc)
      : (col: col, asc: true);
}

/// Comparateur correspondant à `sort` — appliqué à la liste visible.
int Function(Download, Download) downloadComparator(DownloadSort col) {
  return switch (col) {
    DownloadSort.name => (a, b) => a.name.toLowerCase().compareTo(
        b.name.toLowerCase(),
      ),
    DownloadSort.size => (a, b) => a.size.compareTo(b.size),
    DownloadSort.progress => (a, b) => a.progress.compareTo(b.progress),
    DownloadSort.status => (a, b) => a.status.compareTo(b.status),
    DownloadSort.down => (a, b) => a.speedDown.compareTo(b.speedDown),
    DownloadSort.up => (a, b) => a.speedUp.compareTo(b.speedUp),
    DownloadSort.eta => (a, b) => a.etaSeconds.compareTo(b.etaSeconds),
    DownloadSort.peers => (a, b) => a.numConnectedPeers.compareTo(
        b.numConnectedPeers,
      ),
    DownloadSort.ratio => (a, b) => a.ratio.compareTo(b.ratio),
    DownloadSort.added => (a, b) => a.timeAdded.compareTo(b.timeAdded),
  };
}

/// Sélection courante (infohashes) — multi-sélection par cases, plage
/// Shift depuis l'ancre du dernier clic simple.
final downloadSelectionProvider =
    NotifierProvider<DownloadSelectionNotifier, Set<String>>(
      DownloadSelectionNotifier.new,
    );

class DownloadSelectionNotifier extends Notifier<Set<String>> {
  String? _anchor;

  @override
  Set<String> build() => const {};

  void toggle(String infohash) {
    _anchor = infohash;
    state = state.contains(infohash)
        ? state.difference({infohash})
        : state.union({infohash});
  }

  void selectOnly(String infohash) {
    _anchor = infohash;
    state = {infohash};
  }

  /// Sélection par plage (`ordered` = liste visible dans l'ordre
  /// affiché) : de l'ancre à `infohash` inclus.
  void selectRange(String infohash, List<Download> ordered) {
    final from = ordered.indexWhere((d) => d.infohash == _anchor);
    final to = ordered.indexWhere((d) => d.infohash == infohash);
    if (from < 0 || to < 0) {
      selectOnly(infohash);
      return;
    }
    final lo = from < to ? from : to;
    final hi = from < to ? to : from;
    state = {for (var i = lo; i <= hi; i++) ordered[i].infohash};
  }

  /// Clic ligne : `ctrl` = toggle, `shift` = plage, sinon sélection
  /// unique (et nouvelle ancre).
  void click(String infohash, List<Download> ordered,
      {bool ctrl = false, bool shift = false}) {
    if (shift) {
      selectRange(infohash, ordered);
    } else if (ctrl) {
      toggle(infohash);
    } else {
      selectOnly(infohash);
    }
  }

  void selectAll(List<Download> ordered) =>
      state = {for (final d in ordered) d.infohash};

  void clear() {
    _anchor = null;
    state = const {};
  }
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

/// Trackers du téléchargement sélectionné (onglet « Trackers »).
final downloadTrackersProvider = FutureProvider.autoDispose
    .family<List<DownloadTracker>, String>(
      (ref, infohash) =>
          ref.watch(downloadsRepositoryProvider).trackers(infohash),
    );

