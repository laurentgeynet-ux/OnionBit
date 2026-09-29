import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../../downloads/presentation/widgets/add_download_dialog.dart';
import '../../domain/torrent_result.dart';
import '../providers/search_providers.dart';

/// Page « Rechercher » — requête vide = torrents populaires (contenu
/// initial) ; à la frappe, résultats locaux immédiats + résultats
/// distants poussés en continu par SSE.
class SearchPage extends ConsumerWidget {
  const SearchPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final query = ref.watch(searchQueryProvider);
    final sort = ref.watch(searchSortProvider);
    final local = ref.watch(searchResultsProvider);
    final remote = ref.watch(remoteResultsProvider);

    // Fusion local + distant, dédupliquée par info-hash (local d'abord).
    final results = local.value ?? const <TorrentResult>[];
    final merged = <TorrentResult>[
      ...results,
      for (final r in remote.results)
        if (!results.any((l) => l.infohash == r.infohash)) r,
    ];

    final searching = query.isNotEmpty && remote.state.uuid != null;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.sm,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  query.isEmpty
                      ? 'Populaires sur le réseau'
                      : 'Résultats pour « $query »',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
              if (searching)
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const SizedBox(
                      width: 14,
                      height: 14,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                    const SizedBox(width: AppSpacing.sm),
                    Text(
                      'recherche distante '
                      '(${remote.state.peerCount} pairs)',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              PopupMenuButton<SearchSort>(
                tooltip: 'Trier les résultats',
                icon: const Icon(Icons.sort, size: 20),
                initialValue: sort,
                onSelected: (s) =>
                    ref.read(searchSortProvider.notifier).set(s),
                itemBuilder: (context) => [
                  for (final s in SearchSort.values)
                    CheckedPopupMenuItem(
                      value: s,
                      checked: s == sort,
                      child: Text(_sortLabel(s)),
                    ),
                ],
              ),
            ],
          ),
        ),
        Expanded(
          child: local.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              message: '$e',
              onRetry: () => ref.invalidate(searchResultsProvider),
            ),
            data: (_) => merged.isEmpty
                ? EmptyState(
                    icon: Icons.search_off,
                    title: query.isEmpty
                        ? 'Aucun torrent connu'
                        : 'Aucun résultat',
                    message: query.isEmpty
                        ? 'Le daemon découvre encore le réseau — les '
                              'torrents populaires apparaîtront ici.'
                        : 'Essayez d\'autres termes, ou attendez les '
                              'réponses du réseau.',
                  )
                : ListView.builder(
                    padding: const EdgeInsets.symmetric(
                      horizontal: AppSpacing.sm,
                    ),
                    itemCount: merged.length,
                    itemBuilder: (context, i) => _ResultTile(result: merged[i]),
                  ),
          ),
        ),
      ],
    );
  }
}

String _sortLabel(SearchSort s) => switch (s) {
  SearchSort.relevance => 'Pertinence',
  SearchSort.health => 'Santé (seeders)',
  SearchSort.name => 'Nom',
  SearchSort.size => 'Taille',
  SearchSort.date => 'Date',
};

class _ResultTile extends StatelessWidget {
  const _ResultTile({required this.result});

  final TorrentResult result;

  @override
  Widget build(BuildContext context) {
    final r = result;
    final theme = Theme.of(context);
    return ListTile(
      dense: true,
      leading: Icon(
        r.source == TorrentSource.remote
            ? Icons.cloud_outlined
            : Icons.storage_outlined,
        size: 20,
      ),
      title: Text(
        r.name.isEmpty ? r.infohash : r.name,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        [
          ByteFormatter.format(r.size),
          if (r.seeders != null)
            '${r.seeders} seeds / ${r.leechers ?? 0} leechers',
          r.source == TorrentSource.remote ? 'réseau' : 'local',
        ].join(' · '),
        style: theme.textTheme.bodySmall,
      ),
      trailing: FilledButton.tonalIcon(
        onPressed: r.infohash.isEmpty
            ? null
            : () => AddDownloadDialog.show(context, initialUri: r.magnet),
        icon: const Icon(Icons.download, size: 18),
        label: const Text('Ajouter'),
      ),
      onTap: r.infohash.isEmpty
          ? null
          : () => AddDownloadDialog.show(context, initialUri: r.magnet),
    );
  }
}
