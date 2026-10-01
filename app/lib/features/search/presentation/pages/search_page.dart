// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../../../../core/di/providers.dart';
import '../../../../core/l10n/l10n_ext.dart';
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
    final history = ref.watch(searchHistoryProvider);
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
    final l10n = context.l10n;

    // Fusion local + distant, dédupliquée par info-hash (local d'abord).
    final results = local.value ?? const <TorrentResult>[];
    final merged = <TorrentResult>[
      ...results,
      for (final r in remote.results)
        if (!results.any((l) => l.infohash == r.infohash)) r,
    ];
    // Sondes de santé demandées (menu contextuel) — écrasent les
    // seeds/leechers affichés sans recharger la liste.
    final overrides = ref.watch(healthOverridesProvider);
    if (overrides.isNotEmpty) {
      for (var i = 0; i < merged.length; i++) {
        final o = overrides[merged[i].infohash];
        if (o != null) {
          merged[i] = merged[i].withHealth(o.seeders, o.leechers);
        }
      }
    }

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
                          ? l10n.searchPopular
                          : l10n.searchResultsFor(query),
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  if (selection.isNotEmpty) ...[
                    FilledButton.tonalIcon(
                      icon: const Icon(Icons.download, size: 18),
                      label: Text(
                        l10n.searchAddSelected(selection.length),
                      ),
                      onPressed: () =>
                          _addSelected(context, ref, merged, selection),
                    ),
                    const SizedBox(width: AppSpacing.xs),
                    IconButton(
                      tooltip: l10n.searchDeselect,
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
                          l10n.searchRemoteProgress(
                            remote.results.length,
                            remote.state.peerCount,
                          ),
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                        IconButton(
                          tooltip: l10n.searchRemoteStop,
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
                        l10n.searchRemoteFinished(
                          remote.results.length,
                          '${remote.state.finishedAt!.hour.toString().padLeft(2, '0')}'
                              ':${remote.state.finishedAt!.minute.toString().padLeft(2, '0')}',
                        ),
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ),
                  PopupMenuButton<SearchSort>(
                    tooltip: l10n.sortResults,
                    icon: const Icon(Icons.sort, size: 20),
                    initialValue: sort,
                    onSelected: (s) =>
                        ref.read(searchSortProvider.notifier).set(s),
                    itemBuilder: (context) => [
                      for (final s in SearchSort.values)
                        CheckedPopupMenuItem(
                          value: s,
                          checked: s == sort,
                          child: Text(_sortLabel(context.l10n, s)),
                        ),
                    ],
                  ),
                ],
              ),
              if (query.isEmpty && history.isNotEmpty) ...[
                const SizedBox(height: AppSpacing.xs),
                SizedBox(
                  height: 32,
                  child: ListView(
                    scrollDirection: Axis.horizontal,
                    children: [
                      Padding(
                        padding: const EdgeInsets.only(
                          right: AppSpacing.xs,
                          top: 6,
                        ),
                        child: Text(
                          l10n.searchRecent,
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ),
                      for (final h in history)
                        Padding(
                          padding: const EdgeInsets.only(right: AppSpacing.xs),
                          child: ActionChip(
                            label: Text(h),
                            avatar: const Icon(Icons.history, size: 16),
                            onPressed: () =>
                                ref.read(searchQueryProvider.notifier).set(h),
                          ),
                        ),
                    ],
                  ),
                ),
              ],
              const SizedBox(height: AppSpacing.xs),
              SizedBox(
                height: 32,
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  children: [
                    for (final (label, source) in [
                      (l10n.filterAll, null),
                      (l10n.sourceLocal, TorrentSource.local),
                      (l10n.searchSourceNetwork, TorrentSource.remote),
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
                      label: Text(l10n.searchMinSeeds),
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
                        ? l10n.emptyNoTorrents
                        : l10n.emptyNoResults,
                    message: query.isEmpty
                        ? l10n.emptyDiscovering
                        : l10n.emptyTryOther,
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
            ? context.l10n.batchAdded(added)
            : context.l10n.batchAddedFailed(added, failed),
      ),
    ),
  );
}

String _sortLabel(AppLocalizations l10n, SearchSort s) => switch (s) {
  SearchSort.relevance => l10n.sortRelevance,
  SearchSort.health => l10n.sortHealth,
  SearchSort.name => l10n.colName,
  SearchSort.size => l10n.colSize,
  SearchSort.date => l10n.colDate,
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
              context.l10n.resultSeedsLeechers(
                r.seeders!,
                r.leechers ?? 0,
              ),
            r.source == TorrentSource.remote
                ? context.l10n.sourceNetwork
                : context.l10n.sourceLocal,
          ].join(' · '),
          style: theme.textTheme.bodySmall,
        ),
        trailing: FilledButton.tonalIcon(
          onPressed: r.infohash.isEmpty
              ? null
              : () => AddDownloadDialog.show(context, initialUri: r.magnet),
          icon: const Icon(Icons.download, size: 18),
          label: Text(context.l10n.add),
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
    // LayoutBuilder : la largeur utile est celle de la zone de
    // contenu (fenêtre − sidebar), pas `MediaQuery` — la dernière
    // colonne dépassait sinon de ~216 px hors de l'écran.
    return LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minWidth: _minWidth,
            maxWidth: constraints.maxWidth.clamp(_minWidth, 4000.0),
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
          h(context.l10n.colName, SearchCol.name, flex: 5),
          h(context.l10n.colSize, SearchCol.size, width: 90),
          h(context.l10n.colSeeds, SearchCol.seeds, width: 80),
          h(context.l10n.colLeechers, SearchCol.leechers, width: 80),
          h(context.l10n.colDate, SearchCol.date, width: 100),
          h(context.l10n.colSource, SearchCol.source, width: 80),
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
                  r.source == TorrentSource.remote
                      ? context.l10n.sourceNetwork
                      : context.l10n.sourceLocal,
                  style: small,
                ),
              ),
              SizedBox(
                width: 90,
                child: Align(
                  alignment: Alignment.centerRight,
                  child: known.contains(r.infohash)
                      ? Tooltip(
                          message: context.l10n.alreadyDownloaded,
                          child: Chip(
                            label: Text(context.l10n.inProgress),
                            visualDensity: VisualDensity.compact,
                          ),
                        )
                      : IconButton(
                          tooltip: context.l10n.add,
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
        ? (Colors.green, context.l10n.healthSeeders(r.seeders!))
        : (r.leechers ?? 0) > 0
        ? (Colors.orange, context.l10n.healthLeechersOnly)
        : (Colors.grey, context.l10n.healthUnknown);
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
      if (!mounted) return;
      _toast(
        hops == 0
            ? context.l10n.toastAdded(r.name)
            : context.l10n.toastAddedAnon(r.name, hops),
      );
    } catch (e) {
      if (!mounted) return;
      _toast(context.l10n.toastAddError('$e'));
    }
  }

  @override
  Widget build(BuildContext context) {
    final r = _target;
    return MenuAnchor(
      controller: _controller,
      menuChildren: r == null ? const [] : _items(r),
      // Le menu vit dans un overlay : tout pointeur atteignant la
      // liste en dessous est « hors du menu » → fermeture (le clic
      // extérieur ne renvoyait pas de tap géré par le TapRegion dans
      // ce contexte de fenêtrage).
      child: Listener(
        behavior: HitTestBehavior.translucent,
        onPointerDown: (_) {
          if (_controller.isOpen) _controller.close();
        },
        child: widget.child,
      ),
    );
  }

  List<Widget> _items(TorrentResult r) {
    final l10n = context.l10n;
    MenuItemButton item(IconData icon, String label, void Function() onTap) =>
        MenuItemButton(
          leadingIcon: Icon(icon, size: 18),
          onPressed: onTap,
          child: Text(label),
        );

    return [
      item(
        Icons.download,
        l10n.ctxAdd,
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
                h == 0 ? l10n.ctxDirectHop : l10n.ctxHops(h),
              ),
            ),
        ],
        child: Text(l10n.ctxQuickAdd),
      ),
      item(Icons.monitor_heart_outlined, l10n.ctxRefreshHealth, () async {
        try {
          final res = await ref
              .read(healthOverridesProvider.notifier)
              .probe(r.infohash);
          if (!context.mounted) return;
          _toast(
            res == 'ok' ? l10n.toastHealthOk : l10n.toastHealthPending,
          );
        } catch (e) {
          if (!context.mounted) return;
          _toast(l10n.toastHealthError('$e'));
        }
      }),
      const Divider(height: 1),
      item(Icons.link, l10n.ctxCopyMagnet, () {
        Clipboard.setData(ClipboardData(text: r.magnet));
        _toast(l10n.toastMagnetCopied);
      }),
      item(Icons.copy, l10n.ctxCopyInfohash, () {
        Clipboard.setData(ClipboardData(text: r.infohash));
        _toast(l10n.toastInfohashCopied);
      }),
    ];
  }
}
