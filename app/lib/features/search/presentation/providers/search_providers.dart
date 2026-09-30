import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_search_repository.dart';
import '../../domain/search_repository.dart';
import '../../domain/torrent_result.dart';

final searchRepositoryProvider = Provider<SearchRepository>(
  (ref) => RestSearchRepository(ref.watch(apiClientProvider)),
);

/// Tri de la recherche locale — colonnes `sort_by` du backend
/// (`json2pony_columns` : `HEALTH` = seeders/leechers joints,
/// `null` = pertinence FTS `search_rank`).
enum SearchSort {
  /// Pertinence (`search_rank` — titre × santé × fraîcheur).
  relevance(null),

  /// Santé du swarm (`HEALTH` — seeders puis leechers décroissants).
  health('HEALTH'),

  /// Titre alphabétique (`name` → `title COLLATE NOCASE`).
  name('name'),

  /// Taille du contenu (`size`).
  size('size'),

  /// Date du torrent (`date` → `torrent_date`).
  date('date');

  const SearchSort(this.param);

  /// Valeur du paramètre REST `sort_by` (`null` = tri par défaut).
  final String? param;
}

final searchSortProvider = NotifierProvider<SearchSortNotifier, SearchSort>(
  SearchSortNotifier.new,
);

class SearchSortNotifier extends Notifier<SearchSort> {
  @override
  SearchSort build() => SearchSort.relevance;

  void set(SearchSort sort) => state = sort;
}

/// Colonnes triables de la table desktop — tri côté client appliqué à
/// la liste fusionnée (local + distant) ; `null` = ordre brut
/// (pertinence locale puis arrivées distantes).
enum SearchCol { name, size, seeds, leechers, date, source }

final searchColSortProvider =
    NotifierProvider<SearchColSortNotifier, ({SearchCol col, bool asc})?>(
      SearchColSortNotifier.new,
    );

class SearchColSortNotifier extends Notifier<({SearchCol col, bool asc})?> {
  @override
  ({SearchCol col, bool asc})? build() => null;

  void tap(SearchCol col) => state = state?.col == col
      ? (col: col, asc: !state!.asc)
      : (col: col, asc: true);
}

/// Comparateur associé à `col`.
int Function(TorrentResult, TorrentResult) searchComparator(SearchCol col) {
  int? seedsOf(TorrentResult r) => r.seeders;
  int? leechersOf(TorrentResult r) => r.leechers;
  return switch (col) {
    SearchCol.name => (a, b) => a.name.toLowerCase().compareTo(
      b.name.toLowerCase(),
    ),
    SearchCol.size => (a, b) => a.size.compareTo(b.size),
    SearchCol.seeds => (a, b) => (seedsOf(a) ?? -1).compareTo(seedsOf(b) ?? -1),
    SearchCol.leechers => (a, b) => (leechersOf(a) ?? -1).compareTo(
      leechersOf(b) ?? -1,
    ),
    SearchCol.date => (a, b) => (a.date ?? DateTime(1970)).compareTo(
      b.date ?? DateTime(1970),
    ),
    SearchCol.source => (a, b) => a.source.index.compareTo(b.source.index),
  };
}

/// Résultats « de base » : populaires quand la requête est vide,
/// recherche locale sinon. La recherche distante est lancée ici, ses
/// résultats arrivent dans [remoteResultsProvider] via SSE.
final searchResultsProvider =
    AsyncNotifierProvider<SearchResultsNotifier, List<TorrentResult>>(
      SearchResultsNotifier.new,
    );

class SearchResultsNotifier extends AsyncNotifier<List<TorrentResult>> {
  @override
  Future<List<TorrentResult>> build() async {
    final query = ref.watch(searchQueryProvider);
    final sort = ref.watch(searchSortProvider);
    final repo = ref.watch(searchRepositoryProvider);
    if (query.isEmpty) return repo.popular();
    unawaited(_launchRemote(query));
    return repo.searchLocal(query, sortBy: sort.param);
  }

  Future<void> _launchRemote(String query) async {
    try {
      final req = await ref.read(searchRepositoryProvider).searchRemote(query);
      ref.read(remoteResultsProvider.notifier).begin(req);
      await _collectRemote(query);
    } catch (_) {
      // Stack IPv8 inactive ou daemon sans community de découverte :
      // la recherche distante est silencieusement indisponible.
      ref.read(remoteResultsProvider.notifier).idle();
    }
  }

  /// Le backend Rust intègre les `SelectResponse` dans `channel_node`
  /// sans pousser `remote_query_results` : on re-sonde la recherche
  /// locale tant que la requête distante est en vol pour capter les
  /// nouvelles entrées (marquées « réseau »).
  Future<void> _collectRemote(String query) async {
    const attempts = 5;
    const gap = Duration(seconds: 2);
    final repo = ref.read(searchRepositoryProvider);
    for (var i = 0; i < attempts; i++) {
      await Future<void>.delayed(gap);
      if (!ref.mounted || ref.read(searchQueryProvider) != query) return;
      try {
        final fresh = await repo.searchLocal(query);
        ref.read(remoteResultsProvider.notifier).absorb(fresh);
      } catch (_) {
        return;
      }
    }
    ref.read(remoteResultsProvider.notifier).finish();
  }
}

/// État de la recherche distante en cours.
class RemoteSearchState {
  const RemoteSearchState({required this.uuid, required this.peerCount});

  /// `null` = aucune recherche distante en vol.
  final String? uuid;
  final int peerCount;
}

/// Résultats distants accumulés (SSE `remote_query_results`), filtrés
/// sur l'UUID de la requête en cours, dédupliqués par info-hash.
final remoteResultsProvider =
    NotifierProvider<RemoteResultsNotifier, RemoteResults>(
      RemoteResultsNotifier.new,
    );

class RemoteResults {
  const RemoteResults({required this.state, required this.results});

  final RemoteSearchState state;
  final List<TorrentResult> results;
}

class RemoteResultsNotifier extends Notifier<RemoteResults> {
  String? _uuid;

  @override
  RemoteResults build() {
    ref.listen(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event == null || event.topic != EventTopics.remoteQueryResults) {
        return;
      }
      if (_uuid == null || event.data['uuid'] != _uuid) return;
      final incoming = RestSearchRepository.parseRemoteResults(event.data);
      final known = state.results.map((r) => r.infohash).toSet();
      final fresh = [
        for (final r in incoming)
          if (r.infohash.isNotEmpty && !known.contains(r.infohash)) r,
      ];
      if (fresh.isNotEmpty) {
        state = RemoteResults(
          state: state.state,
          results: [...state.results, ...fresh],
        );
      }
    });
    return const RemoteResults(
      state: RemoteSearchState(uuid: null, peerCount: 0),
      results: [],
    );
  }

  /// Démarre une nouvelle collecte pour la requête distante `req`.
  void begin(RemoteQuery req) {
    _uuid = req.requestUuid;
    state = RemoteResults(
      state: RemoteSearchState(
        uuid: req.requestUuid,
        peerCount: req.peers.length,
      ),
      results: const [],
    );
  }

  /// Absorbe des candidats issus d'un re-sondage de la base locale
  /// (réponses distantes intégrées à `channel_node`) — les nouveaux
  /// info-hashes sont marqués « réseau ».
  void absorb(List<TorrentResult> candidates) {
    final known = state.results.map((r) => r.infohash).toSet();
    final fresh = [
      for (final r in candidates)
        if (r.infohash.isNotEmpty && !known.contains(r.infohash)) r.asRemote(),
    ];
    if (fresh.isNotEmpty) {
      state = RemoteResults(
        state: state.state,
        results: [...state.results, ...fresh],
      );
    }
  }

  /// Fin de la fenêtre de collecte (les résultats restent affichés).
  void finish() {
    if (state.state.uuid == null) return;
    state = RemoteResults(
      state: RemoteSearchState(uuid: null, peerCount: state.state.peerCount),
      results: state.results,
    );
  }

  /// Aucune recherche distante en vol (échec du PUT, stack inactive).
  void idle() {
    _uuid = null;
    state = RemoteResults(
      state: const RemoteSearchState(uuid: null, peerCount: 0),
      results: state.results,
    );
  }
}
