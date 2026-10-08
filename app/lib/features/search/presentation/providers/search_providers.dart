// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/config/ui_prefs.dart';
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

  void init(({SearchCol col, bool asc})? v) => state = v;

  void tap(SearchCol col) {
    state = state?.col == col
        ? (col: col, asc: !state!.asc)
        : (col: col, asc: true);
    final s = state;
    unawaited(
      uiPrefsWrite(
        'ui.searchColSort',
        s == null ? '' : '${s.col.name}:${s.asc ? 'asc' : 'desc'}',
      ),
    );
  }
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

/// Filtres rapides appliqués à la liste fusionnée (chips de la page).
class SearchFilter {
  const SearchFilter({this.source, this.minSeeds = false});

  /// `null` = toutes sources.
  final TorrentSource? source;

  /// N'afficher que les résultats à ≥ 10 seeders.
  final bool minSeeds;

  bool get isActive => source != null || minSeeds;

  bool matches(TorrentResult r) {
    if (source != null && r.source != source) return false;
    if (minSeeds && (r.seeders ?? 0) < 10) return false;
    return true;
  }

  SearchFilter copy({TorrentSource? Function()? source, bool? minSeeds}) =>
      SearchFilter(
        source: source != null ? source() : this.source,
        minSeeds: minSeeds ?? this.minSeeds,
      );
}

final searchFilterProvider =
    NotifierProvider<SearchFilterNotifier, SearchFilter>(
      SearchFilterNotifier.new,
    );

class SearchFilterNotifier extends Notifier<SearchFilter> {
  @override
  SearchFilter build() => const SearchFilter();

  void setSource(TorrentSource? s) => state = state.copy(source: () => s);

  void toggleMinSeeds() => state = state.copy(minSeeds: !state.minSeeds);
}

/// Santés rafraîchies à la demande (sonde `/metadata/torrents/{ih}/
/// health`) — écrase les seeds/leechers des résultats affichés sans
/// recharger la liste.
final healthOverridesProvider =
    NotifierProvider<
      HealthOverridesNotifier,
      Map<String, ({int seeders, int leechers})>
    >(HealthOverridesNotifier.new);

class HealthOverridesNotifier
    extends Notifier<Map<String, ({int seeders, int leechers})>> {
  @override
  Map<String, ({int seeders, int leechers})> build() {
    // `torrent_health_updated` (SSE) : santés remontées par le checker
    // et par le gossip content-discovery pour les torrents affichés —
    // met à jour la pastille sans re-sondage `/health`.
    ref.listen(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event == null || event.topic != EventTopics.torrentHealthUpdated) {
        return;
      }
      final ih = '${event.data['infohash'] ?? ''}';
      if (ih.isEmpty) return;
      final s = (event.data['seeders'] as num?)?.toInt();
      final l = (event.data['leechers'] as num?)?.toInt();
      if (s == null && l == null) return;
      state = {...state, ih: (seeders: s ?? 0, leechers: l ?? 0)};
    });
    return const {};
  }

  /// Sonde un infohash — met à jour l'override si le daemon a une
  /// santé connue (`null` = « checking », pas encore de réponse).
  Future<String> probe(String infohash) async {
    final h = await ref.read(searchRepositoryProvider).health(infohash);
    if (h == null) return 'checking';
    state = {...state, infohash: h};
    return 'ok';
  }
}

/// Historique des recherches de la session (LRU, 10 max) — chips
/// « Récents » affichées quand la requête est vide.
final searchHistoryProvider =
    NotifierProvider<SearchHistoryNotifier, List<String>>(
      SearchHistoryNotifier.new,
    );

class SearchHistoryNotifier extends Notifier<List<String>> {
  static const _max = 10;

  @override
  List<String> build() => const [];

  /// Pousse `q` en tête (dédupliqué) — ignoré si déjà en tête.
  void record(String q) {
    if (q.isEmpty || (state.isNotEmpty && state.first == q)) return;
    state = [q, ...state.where((e) => e != q)].take(_max).toList();
  }
}

/// Sélection courante de résultats (infohashes) — ajout en lot.
final searchSelectionProvider =
    NotifierProvider<SearchSelectionNotifier, Set<String>>(
      SearchSelectionNotifier.new,
    );

class SearchSelectionNotifier extends Notifier<Set<String>> {
  @override
  Set<String> build() => const {};

  void toggle(String infohash) => state = state.contains(infohash)
      ? state.difference({infohash})
      : state.union({infohash});

  void selectOnly(String infohash) => state = {infohash};

  void selectAll(List<TorrentResult> results) => state = {
    for (final r in results)
      if (r.infohash.isNotEmpty) r.infohash,
  };

  void clear() => state = const {};
}

/// Résultats « de base » : liste vide quand la requête est vide (les
/// résultats distants ne sont plus persistés — pas de catalogue
/// populaire local), recherche locale sinon. La recherche distante
/// est lancée ici, ses résultats arrivent dans [remoteResultsProvider]
/// via SSE `remote_query_results`.
final searchResultsProvider =
    AsyncNotifierProvider<SearchResultsNotifier, List<TorrentResult>>(
      SearchResultsNotifier.new,
    );

class SearchResultsNotifier extends AsyncNotifier<List<TorrentResult>> {
  /// Fenêtre de collecte des réponses distantes : le backend ne
  /// signale pas de fin explicite, on clot la phase « en cours »
  /// après ce délai (les résultats restent affichés).
  static const _collectWindow = Duration(seconds: 12);

  @override
  Future<List<TorrentResult>> build() async {
    final query = ref.watch(searchQueryProvider);
    final sort = ref.watch(searchSortProvider);
    final repo = ref.watch(searchRepositoryProvider);
    if (query.isEmpty) return const [];
    ref.read(searchHistoryProvider.notifier).record(query);
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

  /// Les `SelectResponse` arrivent en push via `remote_query_results`
  /// (callback `send_search_request` → SSE) et sont accumulées par
  /// [RemoteResultsNotifier] ; rien n'est persisté côté daemon, donc
  /// il n'y a plus de re-sondage local à faire — on attend juste la
  /// fin de la fenêtre de collecte.
  Future<void> _collectRemote(String query) async {
    await Future<void>.delayed(_collectWindow);
    if (!ref.mounted ||
        ref.read(searchQueryProvider) != query ||
        !ref.read(remoteResultsProvider).state.running) {
      return;
    }
    ref.read(remoteResultsProvider.notifier).finish();
  }
}

/// État de la recherche distante en cours.
class RemoteSearchState {
  const RemoteSearchState({
    required this.uuid,
    required this.peerCount,
    this.finishedAt,
  });

  /// `null` = aucune recherche distante en vol.
  final String? uuid;
  final int peerCount;

  /// Fin de la dernière fenêtre de collecte (`null` = jamais lancée).
  final DateTime? finishedAt;

  bool get running => uuid != null;
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

  /// Fin de la fenêtre de collecte (les résultats restent affichés).
  void finish() {
    if (state.state.uuid == null) return;
    state = RemoteResults(
      state: RemoteSearchState(
        uuid: null,
        peerCount: state.state.peerCount,
        finishedAt: DateTime.now(),
      ),
      results: state.results,
    );
  }

  /// Arrêt demandé par l'utilisateur : identique à `finish` — la boucle
  /// `_collectRemote` sort au prochain tick (uuid redevenu `null`).
  void stop() => finish();

  /// Aucune recherche distante en vol (échec du PUT, stack inactive).
  void idle() {
    _uuid = null;
    state = RemoteResults(
      state: const RemoteSearchState(uuid: null, peerCount: 0),
      results: state.results,
    );
  }
}
