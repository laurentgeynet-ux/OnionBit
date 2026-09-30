import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../../../core/layout/breakpoints.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../../downloads/domain/download.dart';
import '../../../downloads/presentation/providers/downloads_providers.dart';
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
    final colSort = ref.watch(searchColSortProvider);
    final selection = ref.watch(searchSelectionProvider);
    final filter = ref.watch(searchFilterProvider);
    final local = ref.watch(searchResultsProvider);
    final remote = ref.watch(remoteResultsProvider);
    // Info-hashes déjà gérés par le daemon — badge « En cours » et
    // bouton Ajouter désactivé (anti-doublon).
    final known = {
      for (final d in ref.watch(downloadsProvider).value ?? const <Download>[])
        d.infohash,
    };
    final compact =
        AppBreakpoints.of(MediaQuery.sizeOf(context).width) ==
        AppBreakpoint.compact;

    // Fusion local + distant, dédupliquée par info-hash (local d'abord).
    final results = local.value ?? const <TorrentResult>[];
    final merged = <TorrentResult>[
      ...results,
      for (final r in remote.results)
        if (!results.any((l) => l.infohash == r.infohash)) r,
    ];
    // Filtres chips (source, seeds min) puis tri colonne.
    if (filter.isActive) {
      merged.removeWhere((r) => !filter.matches(r));
    }
    if (colSort != null) {
      merged.sort(
        (a, b) => searchComparator(colSort.col)(a, b) * (colSort.asc ? 1 : -1),
      );
    }

    final searching = query.isNotEmpty && remote.state.uuid != null;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.sm,
          ),
          child: Column(
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      query.isEmpty
                          ? 'Populaires sur le réseau'
                          : 'Résultats pour « $query »',
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  if (selection.isNotEmpty) ...[
                    FilledButton.tonalIcon(
                      icon: const Icon(Icons.download, size: 18),
                      label: Text('Ajouter (${selection.length})'),
                      onPressed: () =>
                          _addSelected(context, ref, merged, selection),
                    ),
                    const SizedBox(width: AppSpacing.xs),
                    IconButton(
                      tooltip: 'Désélectionner',
                      onPressed: ref
                          .read(searchSelectionProvider.notifier)
                          .clear,
                      icon: const Icon(Icons.close, size: 18),
                    ),
                    const SizedBox(width: AppSpacing.xs),
                  ],
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
                          'recherche distante · '
                          '${remote.results.length} résultat(s) · '
                          '${remote.state.peerCount} pairs',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                        IconButton(
                          tooltip: 'Arrêter la recherche distante',
                          onPressed: ref
                              .read(remoteResultsProvider.notifier)
                              .stop,
                          icon: const Icon(Icons.stop, size: 18),
                        ),
                      ],
                    )
                  else if (remote.state.finishedAt != null &&
                      remote.results.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(right: AppSpacing.xs),
                      child: Text(
                        '${remote.results.length} résultat(s) distants · '
                        'terminée à ${remote.state.finishedAt!.hour.toString().padLeft(2, '0')}'
                        ':${remote.state.finishedAt!.minute.toString().padLeft(2, '0')}',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
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
              const SizedBox(height: AppSpacing.xs),
              SizedBox(
                height: 32,
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  children: [
                    for (final (label, source) in const [
                      ('Tous', null),
                      ('Local', TorrentSource.local),
                      ('Réseau', TorrentSource.remote),
                    ])
                      Padding(
                        padding: const EdgeInsets.only(right: AppSpacing.xs),
                        child: ChoiceChip(
                          label: Text(label),
                          selected: filter.source == source,
                          onSelected: (_) => ref
                              .read(searchFilterProvider.notifier)
                              .setSource(source),
                        ),
                      ),
                    FilterChip(
                      label: const Text('≥ 10 seeds'),
                      selected: filter.minSeeds,
                      onSelected: (_) => ref
                          .read(searchFilterProvider.notifier)
                          .toggleMinSeeds(),
                    ),
                  ],
                ),
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
                : _SearchContextMenu(
                    child: compact
                        ? ListView.builder(
                            padding: const EdgeInsets.symmetric(
                              horizontal: AppSpacing.sm,
                            ),
                            itemCount: merged.length,
                            itemBuilder: (context, i) => _ResultTile(
                              result: merged[i],
                              known: known,
                              selection: selection,
                              query: query,
                            ),
                          )
                        : _ResultsTable(
                            results: merged,
                            known: known,
                            selection: selection,
                            query: query,
                          ),
                  ),
          ),
        ),
      ],
    );
  }
}

/// `TextSpan` avec les termes de `query` en gras — correspondance
/// insensible à la casse, mot par mot.
InlineSpan _highlighted(BuildContext context, String text, String query) {
  final terms = [
    for (final t in query.trim().split(RegExp(r'\s+')))
      if (t.length >= 2) t.toLowerCase(),
  ];
  if (terms.isEmpty || text.isEmpty) return TextSpan(text: text);
  final lower = text.toLowerCase();
  final spans = <TextSpan>[];
  var pos = 0;
  while (pos < text.length) {
    // Prochaine occurrence de n'importe quel terme.
    var hit = -1, len = 0;
    for (final t in terms) {
      final i = lower.indexOf(t, pos);
      if (i >= 0 && (hit < 0 || i < hit)) {
        hit = i;
        len = t.length;
      }
    }
    if (hit < 0) {
      spans.add(TextSpan(text: text.substring(pos)));
      break;
    }
    if (hit > pos) spans.add(TextSpan(text: text.substring(pos, hit)));
    spans.add(
      TextSpan(
        text: text.substring(hit, hit + len),
        style: const TextStyle(fontWeight: FontWeight.bold),
      ),
    );
    pos = hit + len;
  }
  return TextSpan(
    style: Theme.of(context).textTheme.bodyMedium,
    children: spans,
  );
}

/// Ajout en lot de la sélection — erreurs par élément, snackbar de
/// synthèse, sélection vidée au terme.
Future<void> _addSelected(
  BuildContext context,
  WidgetRef ref,
  List<TorrentResult> merged,
  Set<String> selection,
) async {
  final repo = ref.read(downloadsRepositoryProvider);
  var added = 0, failed = 0;
  for (final r in merged.where((e) => selection.contains(e.infohash))) {
    try {
      await repo.add(uri: r.magnet);
      added++;
    } catch (_) {
      failed++;
    }
  }
  ref.read(searchSelectionProvider.notifier).clear();
  await ref.read(downloadsProvider.notifier).refresh();
  if (!context.mounted) return;
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        failed == 0
            ? '$added téléchargement(s) ajouté(s)'
            : '$added ajouté(s), $failed en échec',
      ),
    ),
  );
}

String _sortLabel(SearchSort s) => switch (s) {
  SearchSort.relevance => 'Pertinence',
  SearchSort.health => 'Santé (seeders)',
  SearchSort.name => 'Nom',
  SearchSort.size => 'Taille',
  SearchSort.date => 'Date',
};

class _ResultTile extends StatelessWidget {
  const _ResultTile({
    required this.result,
    required this.known,
    required this.selection,
    required this.query,
  });

  final TorrentResult result;
  final String query;

  /// Info-hashes déjà gérés par le daemon.
  final Set<String> known;
  final Set<String> selection;

  @override
  Widget build(BuildContext context) {
    final r = result;
    final theme = Theme.of(context);
    return GestureDetector(
      onSecondaryTapUp: (details) =>
          _SearchContextMenu.show(context, details.globalPosition, r),
      child: ListTile(
        dense: true,
        leading: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Checkbox(
              value: selection.contains(r.infohash),
              visualDensity: VisualDensity.compact,
              onChanged: r.infohash.isEmpty
                  ? null
                  : (_) =>
                        ProviderScope.containerOf(context)
                            .read(searchSelectionProvider.notifier)
                            .toggle(r.infohash),
            ),
            Icon(
              r.source == TorrentSource.remote
                  ? Icons.cloud_outlined
                  : Icons.storage_outlined,
              size: 20,
            ),
          ],
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
      ),
    );
  }
}

/// Table desktop : en-têtes triables (clic = asc/desc, comme la page
/// Téléchargements) + scroll horizontal sous ~900 px utiles.
class _ResultsTable extends ConsumerWidget {
  const _ResultsTable({
    required this.results,
    required this.known,
    required this.selection,
    required this.query,
  });

  final List<TorrentResult> results;
  final Set<String> known;
  final Set<String> selection;
  final String query;

  static const double _minWidth = 900;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: ConstrainedBox(
        constraints: BoxConstraints(
          minWidth: _minWidth,
          maxWidth: MediaQuery.sizeOf(context).width
              .clamp(_minWidth, 4000)
              .toDouble(),
        ),
        child: Column(
          children: [
            const _HeaderRow(),
            const Divider(height: 1),
            Expanded(
              child: ListView.builder(
                itemCount: results.length,
                itemBuilder: (context, i) => _ResultRow(
                  result: results[i],
                  known: known,
                  selection: selection,
                  query: query,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _HeaderRow extends ConsumerWidget {
  const _HeaderRow();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final style = Theme.of(context).textTheme.labelSmall;
    final sort = ref.watch(searchColSortProvider);
    final notifier = ref.read(searchColSortProvider.notifier);
    final scheme = Theme.of(context).colorScheme;

    Widget h(String s, SearchCol col, {double? width, int flex = 0}) {
      final active = sort?.col == col;
      final child = InkWell(
        onTap: () => notifier.tap(col),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Flexible(
              child: Text(
                s,
                style: style?.copyWith(
                  color: active ? scheme.primary : null,
                  fontWeight: active ? FontWeight.w600 : null,
                ),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (active)
              Icon(
                sort!.asc ? Icons.arrow_upward : Icons.arrow_downward,
                size: 12,
                color: scheme.primary,
              ),
          ],
        ),
      );
      return flex > 0
          ? Expanded(flex: flex, child: child)
          : SizedBox(width: width, child: child);
    }

    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: AppSpacing.sm,
        vertical: AppSpacing.xs,
      ),
      child: Row(
        children: [
          const SizedBox(width: 36),
          h('Nom', SearchCol.name, flex: 5),
          h('Taille', SearchCol.size, width: 90),
          h('Seeds', SearchCol.seeds, width: 80),
          h('Leechers', SearchCol.leechers, width: 80),
          h('Date', SearchCol.date, width: 100),
          h('Source', SearchCol.source, width: 80),
          const SizedBox(width: 90),
        ],
      ),
    );
  }
}

class _ResultRow extends StatelessWidget {
  const _ResultRow({
    required this.result,
    required this.known,
    required this.selection,
    required this.query,
  });

  final TorrentResult result;
  final Set<String> known;
  final Set<String> selection;
  final String query;

  @override
  Widget build(BuildContext context) {
    final r = result;
    final small = Theme.of(context).textTheme.bodySmall;
    return Material(
      child: InkWell(
        onTap: r.infohash.isEmpty
            ? null
            : () => AddDownloadDialog.show(context, initialUri: r.magnet),
        onSecondaryTapUp: (details) =>
            _SearchContextMenu.show(context, details.globalPosition, r),
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.sm,
            vertical: AppSpacing.xs,
          ),
          child: Row(
            children: [
              SizedBox(
                width: 36,
                child: Checkbox(
                  value: selection.contains(r.infohash),
                  onChanged: r.infohash.isEmpty
                      ? null
                      : (_) =>
                            ProviderScope.containerOf(context)
                                .read(searchSelectionProvider.notifier)
                                .toggle(r.infohash),
                ),
              ),
              Expanded(
                flex: 5,
                child: Text.rich(
                  _highlighted(
                    context,
                    r.name.isEmpty ? r.infohash : r.name,
                    query,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              SizedBox(
                width: 90,
                child: Text(
                  r.size > 0 ? ByteFormatter.format(r.size) : '—',
                  style: small,
                ),
              ),
              SizedBox(
                width: 80,
                child: Row(
                  children: [
                    _ResultHealthDot(result: r),
                    const SizedBox(width: 4),
                    Text('${r.seeders ?? '—'}', style: small),
                  ],
                ),
              ),
              SizedBox(
                width: 80,
                child: Text('${r.leechers ?? '—'}', style: small),
              ),
              SizedBox(
                width: 100,
                child: Text(_formatDate(r.date), style: small),
              ),
              SizedBox(
                width: 80,
                child: Text(
                  r.source == TorrentSource.remote ? 'réseau' : 'local',
                  style: small,
                ),
              ),
              SizedBox(
                width: 90,
                child: Align(
                  alignment: Alignment.centerRight,
                  child: known.contains(r.infohash)
                      ? const Tooltip(
                          message: 'Déjà dans vos téléchargements',
                          child: Chip(
                            label: Text('En cours'),
                            visualDensity: VisualDensity.compact,
                          ),
                        )
                      : IconButton(
                          tooltip: 'Ajouter',
                          onPressed: r.infohash.isEmpty
                              ? null
                              : () => AddDownloadDialog.show(
                                  context,
                                  initialUri: r.magnet,
                                ),
                          icon: const Icon(Icons.download, size: 18),
                        ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Pastille de santé du résultat : vert = seeders, orange = leechers
/// seuls, gris = santé inconnue ou morte.
class _ResultHealthDot extends StatelessWidget {
  const _ResultHealthDot({required this.result});

  final TorrentResult result;

  @override
  Widget build(BuildContext context) {
    final r = result;
    final (color, tip) = (r.seeders ?? 0) > 0
        ? (Colors.green, '${r.seeders} seeder(s)')
        : (r.leechers ?? 0) > 0
        ? (Colors.orange, 'Leechers seuls — santé fragile')
        : (Colors.grey, 'Santé inconnue');
    return Tooltip(
      message: tip,
      child: Container(
        width: 8,
        height: 8,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      ),
    );
  }
}

String _formatDate(DateTime? d) {
  if (d == null) return '—';
  String two(int v) => v.toString().padLeft(2, '0');
  return '${d.year}-${two(d.month)}-${two(d.day)}';
}

/// Menu contextuel d'un résultat (même pattern `MenuAnchor` que la page
/// Téléchargements) : ajout direct anonyme, copie magnet/info-hash.
class _SearchContextMenu extends ConsumerStatefulWidget {
  const _SearchContextMenu({required this.child});

  final Widget child;

  static void show(BuildContext context, Offset position, TorrentResult r) {
    context.findAncestorStateOfType<_SearchContextMenuState>()?._open(
      position,
      r,
    );
  }

  @override
  ConsumerState<_SearchContextMenu> createState() => _SearchContextMenuState();
}

class _SearchContextMenuState extends ConsumerState<_SearchContextMenu> {
  final MenuController _controller = MenuController();
  TorrentResult? _target;

  void _open(Offset position, TorrentResult r) {
    setState(() => _target = r);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _controller.open(position: position);
    });
  }

  void _toast(String message) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  /// Ajout direct (sans dialogue) avec `hops` sauts anonymes —
  /// `safeSeeding` forcé quand hops > 0 (règle Python).
  Future<void> _add(TorrentResult r, int hops) async {
    try {
      await ref
          .read(downloadsRepositoryProvider)
          .add(uri: r.magnet, anonHops: hops, safeSeeding: hops > 0);
      await ref.read(downloadsProvider.notifier).refresh();
      _toast(
        hops == 0
            ? '« ${r.name} » ajouté'
            : '« ${r.name} » ajouté en anonyme ($hops saut${hops > 1 ? 's' : ''})',
      );
    } catch (e) {
      _toast('Ajout : $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final r = _target;
    return MenuAnchor(
      controller: _controller,
      menuChildren: r == null ? const [] : _items(r),
      child: widget.child,
    );
  }

  List<Widget> _items(TorrentResult r) {
    MenuItemButton item(IconData icon, String label, void Function() onTap) =>
        MenuItemButton(
          leadingIcon: Icon(icon, size: 18),
          onPressed: onTap,
          child: Text(label),
        );

    return [
      item(
        Icons.download,
        'Ajouter…',
        () => AddDownloadDialog.show(context, initialUri: r.magnet),
      ),
      SubmenuButton(
        leadingIcon: const Icon(Icons.shield_outlined, size: 18),
        menuChildren: [
          for (final h in const [0, 1, 2, 3])
            MenuItemButton(
              leadingIcon: Icon(
                h == 0 ? Icons.public : Icons.shield_outlined,
                size: 18,
              ),
              onPressed: () => _add(r, h),
              child: Text(
                h == 0 ? 'Direct (0 saut)' : '$h saut${h > 1 ? 's' : ''}',
              ),
            ),
        ],
        child: const Text('Ajout rapide'),
      ),
      const Divider(height: 1),
      item(Icons.link, 'Copier le lien magnet', () {
        Clipboard.setData(ClipboardData(text: r.magnet));
        _toast('Lien magnet copié');
      }),
      item(Icons.copy, 'Copier l\'info-hash', () {
        Clipboard.setData(ClipboardData(text: r.infohash));
        _toast('Info-hash copié');
      }),
    ];
  }
}
