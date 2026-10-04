// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/layout/breakpoints.dart';
import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/download.dart';
import '../../domain/download_filter.dart';
import '../../domain/downloads_repository.dart';
import '../providers/downloads_providers.dart';
import '../widgets/add_download_dialog.dart';
import '../widgets/download_actions.dart';
import '../widgets/download_detail_panel.dart';
import '../widgets/download_status.dart';

/// Page « Téléchargements » — table + panneau de détail (desktop et
/// web large) ; liste compacte + détail en bottom sheet (< 600 dp).
/// Le filtre (`?f=`) vient de l'URL — la sidebar y navigue.
class DownloadsPage extends ConsumerStatefulWidget {
  const DownloadsPage({super.key, required this.filter});

  final DownloadFilter filter;

  @override
  ConsumerState<DownloadsPage> createState() => _DownloadsPageState();
}

class _DownloadsPageState extends ConsumerState<DownloadsPage> {
  String _nameFilter = '';

  @override
  Widget build(BuildContext context) {
    final breakpoint = AppBreakpoints.of(MediaQuery.sizeOf(context).width);
    final compact = breakpoint == AppBreakpoint.compact;
    final async = ref.watch(downloadsProvider);
    final selection = ref.watch(downloadSelectionProvider);

    return Column(
      children: [
        _Toolbar(
          compact: compact,
          selection: selection,
          filter: widget.filter,
          onNameFilter: compact ? null : (v) => setState(() => _nameFilter = v),
        ),
        Expanded(
          // Le glisser-déposer est géré par la `DropZone` globale
          // (`app_shell.dart`) : elle route le fichier vers le dialogue
          // « Ajouter » où l'utilisateur choisit les sauts. Un DropTarget
          // local doublonnerait l'ajout en clair (hops=0).
          child: async.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              error: e,
              onRetry: () => ref.read(downloadsProvider.notifier).refresh(),
            ),
            data: (all) {
              final sort = ref.watch(downloadSortProvider);
              final visible =
                  all
                      .where(widget.filter.matches)
                      .where(
                        (d) =>
                            _nameFilter.isEmpty ||
                            d.name.toLowerCase().contains(
                              _nameFilter.toLowerCase(),
                            ),
                      )
                      .toList()
                    ..sort(
                      (a, b) =>
                          downloadComparator(sort.col)(a, b) *
                          (sort.asc ? 1 : -1),
                    );
              if (visible.isEmpty) {
                final l10n = context.l10n;
                return EmptyState(
                  icon: Icons.download_outlined,
                  title: l10n.noDownloads,
                  message: widget.filter == DownloadFilter.all
                      ? l10n.noDownloadsHint
                      : l10n.filterEmpty(widget.filter.label(l10n)),
                  action: widget.filter == DownloadFilter.all
                      ? FilledButton.icon(
                          onPressed: () => AddDownloadDialog.show(context),
                          icon: const Icon(Icons.add),
                          label: Text(l10n.add),
                        )
                      : null,
                );
              }
              // Le mode d'affichage choisi ne s'applique qu'en
              // desktop : l'écran étroit impose la liste compacte.
              final viewMode = ref.watch(downloadViewModeProvider);
              final effective = compact ? DownloadViewMode.compact : viewMode;
              return _DownloadsContextMenu(
                child: switch (effective) {
                  DownloadViewMode.compact => _CompactList(downloads: visible),
                  DownloadViewMode.grid => CallbackShortcuts(
                    bindings: {
                      const SingleActivator(
                        LogicalKeyboardKey.keyA,
                        control: true,
                      ): () => ref
                          .read(downloadSelectionProvider.notifier)
                          .selectAll(visible),
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          ref.read(downloadSelectionProvider.notifier).clear(),
                    },
                    child: Focus(
                      autofocus: true,
                      child: _GridView(downloads: visible),
                    ),
                  ),
                  DownloadViewMode.table => CallbackShortcuts(
                    bindings: {
                      const SingleActivator(
                        LogicalKeyboardKey.keyA,
                        control: true,
                      ): () => ref
                          .read(downloadSelectionProvider.notifier)
                          .selectAll(visible),
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          ref.read(downloadSelectionProvider.notifier).clear(),
                      const SingleActivator(LogicalKeyboardKey.space): () =>
                          _togglePauseSelection(all, selection),
                      const SingleActivator(LogicalKeyboardKey.delete): () =>
                          confirmRemoveSelected(context, ref, selection),
                      const SingleActivator(LogicalKeyboardKey.f2): () {
                        if (selection.length != 1) return;
                        final d = all
                            .where((e) => e.infohash == selection.first)
                            .firstOrNull;
                        if (d != null) showRateLimitsDialog(context, d);
                      },
                    },
                    child: Focus(
                      autofocus: true,
                      child: _DesktopTable(
                        downloads: visible,
                        selection: selection,
                      ),
                    ),
                  ),
                },
              );
            },
          ),
        ),
        // Panneau de détail (desktop) — une seule ligne sélectionnée.
        if (!compact && selection.length == 1)
          ..._detailPanel(async.value, selection.first),
      ],
    );
  }

  List<Widget> _detailPanel(List<Download>? all, String infohash) {
    final d = all?.where((e) => e.infohash == infohash).firstOrNull;
    if (d == null) return const [];
    final wanted = ref.watch(detailPanelHeightProvider);
    // Plafond effectif : le panneau ne peut pas manger plus de 70 %
    // de la fenêtre (sinon la liste n'a plus de place).
    final height = wanted.clamp(
      DetailPanelHeightNotifier.minHeight,
      MediaQuery.sizeOf(context).height * 0.7,
    );
    return [
      // Poignée de redimensionnement : glisser vertical sur le
      // séparateur ajuste la hauteur du panneau (persistée).
      MouseRegion(
        cursor: SystemMouseCursors.resizeUpDown,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onVerticalDragUpdate: (d) => ref
              .read(detailPanelHeightProvider.notifier)
              .set(wanted - d.delta.dy),
          child: const SizedBox(height: 9, child: Divider(height: 9)),
        ),
      ),
      SizedBox(
        height: height,
        child: DownloadDetailPanel(download: d),
      ),
    ];
  }

  /// Espace : reprend les éléments en pause, met en pause les autres —
  /// sélection mixte gérée élément par élément.
  void _togglePauseSelection(List<Download> all, Set<String> selection) {
    final notifier = ref.read(downloadsProvider.notifier);
    for (final d in all) {
      if (!selection.contains(d.infohash)) continue;
      final future = d.isPaused
          ? notifier.resume(d.infohash)
          : notifier.pause(d.infohash);
      future.catchError((Object e) {
        if (mounted) {
          showDownloadError(context, context.l10n.actPauseResume, e);
        }
        return null;
      });
    }
  }
}

/// Dialogue de suppression partagé entre la barre d'actions et le
/// raccourci Suppr.
Future<void> confirmRemoveSelected(
  BuildContext context,
  WidgetRef ref,
  Set<String> selection,
) async {
  if (selection.isEmpty) return;
  var deleteFiles = false;
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setState) => AlertDialog(
        title: Text(ctx.l10n.deleteTitle),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(ctx.l10n.deleteConfirmBody(selection.length)),
            CheckboxListTile(
              value: deleteFiles,
              onChanged: (v) => setState(() => deleteFiles = v ?? false),
              title: Text(ctx.l10n.deleteAlsoFiles),
              contentPadding: EdgeInsets.zero,
              dense: true,
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(ctx.l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(ctx.l10n.delete),
          ),
        ],
      ),
    ),
  );
  if (ok != true) return;
  final notifier = ref.read(downloadsProvider.notifier);
  try {
    await Future.wait([
      for (final ih in selection) notifier.remove(ih, deleteFiles: deleteFiles),
    ]);
    ref.read(downloadSelectionProvider.notifier).clear();
  } catch (e) {
    if (context.mounted) {
      showDownloadError(context, context.l10n.actDelete, e);
    }
  }
}

/// Barre d'actions : actions de sélection (contextuelles) + filtre
/// par nom (desktop) + chips de filtre (compact).
class _Toolbar extends ConsumerWidget {
  const _Toolbar({
    required this.compact,
    required this.selection,
    required this.filter,
    required this.onNameFilter,
  });

  final bool compact;
  final Set<String> selection;
  final DownloadFilter filter;
  final ValueChanged<String>? onNameFilter;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final notifier = ref.read(downloadsProvider.notifier);
    final sel = ref.read(downloadSelectionProvider.notifier);

    final l10n = context.l10n;

    Future<void> run(
      String label,
      Future<void> Function(String ih) action,
    ) async {
      try {
        await Future.wait([for (final ih in selection) action(ih)]);
      } catch (e) {
        if (context.mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text('$label : $e')));
        }
      }
    }

    Future<void> confirmRemove() =>
        confirmRemoveSelected(context, ref, selection);

    return Material(
      color: Theme.of(context).colorScheme.surface,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: AppSpacing.md,
          vertical: AppSpacing.xs,
        ),
        child: Column(
          children: [
            Row(
              children: [
                if (selection.isEmpty) ...[
                  Flexible(
                    child: Text(
                      l10n.navDownloads,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  const Spacer(),
                  if (!compact)
                    _ViewModeSelector(
                      mode: ref.watch(downloadViewModeProvider),
                    ),
                  if (onNameFilter != null) ...[
                    const SizedBox(width: AppSpacing.sm),
                    SizedBox(
                      width: 180,
                      child: TextField(
                        decoration: InputDecoration(
                          hintText: l10n.filterByName,
                          isDense: true,
                          prefixIcon: const Icon(Icons.filter_list, size: 18),
                          border: const OutlineInputBorder(),
                        ),
                        onChanged: onNameFilter,
                      ),
                    ),
                  ],
                ] else ...[
                  Text(
                    l10n.selectedCount(selection.length),
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                  const SizedBox(width: AppSpacing.md),
                  IconButton(
                    tooltip: l10n.resume,
                    onPressed: () => run(l10n.actResume, notifier.resume),
                    icon: const Icon(Icons.play_arrow),
                  ),
                  IconButton(
                    tooltip: l10n.pause,
                    onPressed: () => run(l10n.actPause, notifier.pause),
                    icon: const Icon(Icons.pause),
                  ),
                  IconButton(
                    tooltip: l10n.delete,
                    onPressed: confirmRemove,
                    icon: const Icon(Icons.delete_outline),
                  ),
                  const Spacer(),
                  IconButton(
                    tooltip: l10n.deselect,
                    onPressed: sel.clear,
                    icon: const Icon(Icons.close),
                  ),
                ],
              ],
            ),
            if (compact) ...[
              const SizedBox(height: AppSpacing.xs),
              SizedBox(
                height: 36,
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  children: [
                    for (final f in DownloadFilter.values)
                      Padding(
                        padding: const EdgeInsets.only(right: AppSpacing.xs),
                        child: ChoiceChip(
                          label: Text(f.label(context.l10n)),
                          selected: f == filter,
                          onSelected: (_) => _goFilter(context, f),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  static void _goFilter(BuildContext context, DownloadFilter f) {
    // Le filtre vit dans l'URL (deep-linkable, synchronisé sidebar).
    context.go(
      f.queryKey.isEmpty ? '/downloads' : '/downloads?f=${f.queryKey}',
    );
  }
}

/// Table desktop : en-tête + lignes custom (scroll horizontal sous
/// ~900 px utiles).
class _DesktopTable extends ConsumerWidget {
  const _DesktopTable({required this.downloads, required this.selection});

  final List<Download> downloads;
  final Set<String> selection;

  static const double _minWidth = 1012;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // LayoutBuilder : la largeur utile est celle de la zone de
    // contenu (fenêtre − sidebar), pas `MediaQuery` — les dernières
    // colonnes dépassaient sinon de ~216 px hors de l'écran.
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
                  itemCount: downloads.length,
                  itemBuilder: (context, i) =>
                      _DownloadRow(download: downloads[i], ordered: downloads),
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
    final l10n = context.l10n;
    final sort = ref.watch(downloadSortProvider);
    final sortNotifier = ref.read(downloadSortProvider.notifier);
    final scheme = Theme.of(context).colorScheme;

    /// En-tête triable : la colonne active affiche la flèche du sens.
    Widget h(String s, DownloadSort col, {double? width, int flex = 0}) {
      final active = sort.col == col;
      final child = InkWell(
        onTap: () => sortNotifier.tap(col),
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
                sort.asc ? Icons.arrow_upward : Icons.arrow_downward,
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
          h(l10n.colName, DownloadSort.name, flex: 4),
          h(l10n.colSize, DownloadSort.size, width: 80),
          h(l10n.colProgress, DownloadSort.progress, flex: 3),
          h('↓', DownloadSort.down, width: 90),
          h('↑', DownloadSort.up, width: 90),
          h('ETA', DownloadSort.eta, width: 80),
          h(l10n.colPeers, DownloadSort.peers, width: 80),
          h(l10n.colRatio, DownloadSort.ratio, width: 70),
          h(l10n.colAdded, DownloadSort.added, width: 100),
          const SizedBox(width: 130),
        ],
      ),
    );
  }
}

class _DownloadRow extends ConsumerWidget {
  const _DownloadRow({required this.download, required this.ordered});

  final Download download;

  /// Liste visible dans l'ordre affiché — nécessaire à la sélection
  /// par plage (Shift+clic).
  final List<Download> ordered;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final d = download;
    final selection = ref.watch(downloadSelectionProvider);
    final sel = ref.read(downloadSelectionProvider.notifier);
    final selected = selection.contains(d.infohash);
    final scheme = Theme.of(context).colorScheme;
    final small = Theme.of(context).textTheme.bodySmall;

    return Material(
      color: selected ? scheme.secondaryContainer : null,
      child: InkWell(
        onTap: () {
          final kb = HardwareKeyboard.instance;
          sel.click(
            d.infohash,
            ordered,
            ctrl: kb.isControlPressed || kb.isMetaPressed,
            shift: kb.isShiftPressed,
          );
        },
        onSecondaryTapUp: (details) =>
            _DownloadsContextMenu.show(context, details.globalPosition, d),
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
                  value: selected,
                  onChanged: (_) => sel.toggle(d.infohash),
                ),
              ),
              Expanded(
                flex: 4,
                child: Text(
                  d.name.isEmpty ? d.infohash : d.name,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              SizedBox(
                width: 80,
                child: Text(context.fmtBytes(d.size), style: small),
              ),
              Expanded(
                flex: 3,
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpacing.sm,
                  ),
                  child: DownloadProgressBar(download: d),
                ),
              ),
              SizedBox(
                width: 90,
                child: Text(context.fmtRate(d.speedDown), style: small),
              ),
              SizedBox(
                width: 90,
                child: Text(context.fmtRate(d.speedUp), style: small),
              ),
              SizedBox(
                width: 80,
                child: Text(context.fmtEta(d.etaSeconds), style: small),
              ),
              SizedBox(
                width: 80,
                child: Row(
                  children: [
                    _HealthDot(download: d),
                    const SizedBox(width: 4),
                    Text(
                      '${d.numConnectedPeers} (${d.numPeers})',
                      style: small,
                    ),
                  ],
                ),
              ),
              SizedBox(
                width: 70,
                child: Text(d.ratio.toStringAsFixed(2), style: small),
              ),
              SizedBox(
                width: 100,
                child: Text(_formatDate(d.timeAdded), style: small),
              ),
              SizedBox(width: 130, child: _RowBadges(download: d)),
            ],
          ),
        ),
      ),
    );
  }
}

/// Pastille de santé de l'essaim : vert = seeders connectés, orange =
/// seeders connus mais aucun connecté, rouge = essaim mort.
class _HealthDot extends StatelessWidget {
  const _HealthDot({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final d = download;
    final l10n = context.l10n;
    final (color, tip) = d.numSeeds <= 0
        ? (Colors.red, l10n.noSeeders)
        : d.numConnectedPeers > 0
        ? (Colors.green, l10n.seedersReachable(d.numSeeds))
        : (Colors.orange, l10n.seedersNoPeers);
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

/// Badges d'état en fin de ligne : erreur, torrent privé, position de
/// file, anonymat. 88 px fixes — les icônes absentes ne prennent pas de
/// place.
class _RowBadges extends StatelessWidget {
  const _RowBadges({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final d = download;
    final scheme = Theme.of(context).colorScheme;
    final style = Theme.of(context).textTheme.labelSmall;
    return Row(
      mainAxisAlignment: MainAxisAlignment.end,
      children: [
        if (d.isError)
          Tooltip(
            message: d.error.isEmpty ? context.l10n.stoppedOnError : d.error,
            child: Icon(Icons.error_outline, size: 16, color: scheme.error),
          ),
        if (d.isPrivate)
          Tooltip(
            message: context.l10n.privateTorrent,
            child: Icon(Icons.lock_outline, size: 16, color: scheme.outline),
          ),
        if (d.queuePosition >= 0)
          Tooltip(
            message: context.l10n.queuePosTip(d.queuePosition),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.format_list_numbered,
                  size: 16,
                  color: scheme.outline,
                ),
                Text('${d.queuePosition}', style: style),
              ],
            ),
          ),
        AnonBadge(download: d),
      ],
    );
  }
}

String _formatDate(int epoch) {
  if (epoch <= 0) return '—';
  final dt = DateTime.fromMillisecondsSinceEpoch(epoch * 1000);
  String two(int v) => v.toString().padLeft(2, '0');
  return '${dt.year}-${two(dt.month)}-${two(dt.day)}';
}

/// Menu contextuel d'un téléchargement (`MenuAnchor` Material 3 +
/// sous-menus `SubmenuButton`) — le clic droit / appui long ouvre le
/// menu au curseur. « File d'attente » et « Anonymat » sont des
/// sous-menus : le menu plat devenait plus haut que la fenêtre et
/// rendait les entrées basses (sauts, suppression) difficiles à
/// atteindre.
class _DownloadsContextMenu extends ConsumerStatefulWidget {
  const _DownloadsContextMenu({required this.child});

  final Widget child;

  /// Ouvre le menu de `d` à `position` (coordonnées globales).
  static void show(BuildContext context, Offset position, Download d) {
    context.findAncestorStateOfType<_DownloadsContextMenuState>()?._open(
      position,
      d,
    );
  }

  @override
  ConsumerState<_DownloadsContextMenu> createState() =>
      _DownloadsContextMenuState();
}

class _DownloadsContextMenuState extends ConsumerState<_DownloadsContextMenu> {
  final MenuController _controller = MenuController();
  Download? _target;

  void _open(Offset position, Download d) {
    setState(() => _target = d);
    // Ouvre après le rebuild : `menuChildren` reflète alors le
    // téléchargement visé (MenuAnchor évalue les enfants à l'ouverture).
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _controller.open(position: position);
    });
  }

  /// Exécute une action et rapporte l'erreur en snackbar.
  void _act(String label, Future<void> future) {
    future.catchError((Object e) {
      if (mounted) showDownloadError(context, label, e);
      return null;
    });
  }

  void _toast(String message) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  /// Presets en octets/s : 64/128/256/512 Kio/s, 1/4 Mio/s.
  static const _kRatePresets = [
    64 * 1024,
    128 * 1024,
    256 * 1024,
    512 * 1024,
    1024 * 1024,
    4 * 1024 * 1024,
  ];

  /// Sous-menu d'une direction (réception/envoi) : presets cochés sur
  /// la valeur courante, « Illimité » (`-1` — `0` est ignoré par le
  /// walrus backend, `-1` retombe sur `None` côté `try_from`), et
  /// « Personnalisé… » renvoyant au dialogue complet.
  Widget _rateLimitSubmenu({
    required String label,
    required int current,
    required List<int> presets,
    required void Function(int bytesPerSec) onPick,
    required void Function() onCustom,
  }) {
    return SubmenuButton(
      menuChildren: [
        for (final v in presets)
          MenuItemButton(
            leadingIcon: Icon(current == v ? Icons.check : null, size: 18),
            onPressed: () => onPick(v),
            child: Text(context.fmtRate(v)),
          ),
        const Divider(height: 1),
        MenuItemButton(
          leadingIcon: Icon(current <= 0 ? Icons.check : null, size: 18),
          onPressed: () => onPick(-1),
          child: Text(context.l10n.rateUnlimited),
        ),
        MenuItemButton(
          leadingIcon: const Icon(Icons.tune, size: 18),
          onPressed: onCustom,
          child: Text(context.l10n.customRate),
        ),
      ],
      child: Text('$label (${current > 0 ? context.fmtRate(current) : '∞'})'),
    );
  }

  @override
  Widget build(BuildContext context) {
    final d = _target;
    return MenuAnchor(
      controller: _controller,
      menuChildren: d == null ? const [] : _items(d),
      // Le menu vit dans un overlay : tout pointeur qui atteint la
      // liste en dessous est par définition « hors du menu » → on
      // ferme. Les clics sur les items du menu ne traversent pas
      // l'overlay, ce Listener ne les voit jamais.
      child: Listener(
        behavior: HitTestBehavior.translucent,
        onPointerDown: (_) {
          if (_controller.isOpen) _controller.close();
        },
        child: widget.child,
      ),
    );
  }

  List<Widget> _items(Download d) {
    final l10n = context.l10n;
    final notifier = ref.read(downloadsProvider.notifier);
    final magnetUri =
        'magnet:?xt=urn:btih:${d.infohash}&dn=${Uri.encodeComponent(d.name.isEmpty ? d.infohash : d.name)}';

    MenuItemButton item(IconData icon, String label, void Function() onTap) =>
        MenuItemButton(
          leadingIcon: Icon(icon, size: 18),
          onPressed: onTap,
          child: Text(label),
        );

    return [
      item(
        d.isPaused ? Icons.play_arrow : Icons.pause,
        d.isPaused ? l10n.resume : l10n.pause,
        () => _act(
          d.isPaused ? l10n.actResume : l10n.actPause,
          d.isPaused ? notifier.resume(d.infohash) : notifier.pause(d.infohash),
        ),
      ),
      if (d.destination.isNotEmpty)
        item(Icons.folder_open, l10n.openFolder, () => openPath(d.destination)),
      const Divider(height: 1),
      SubmenuButton(
        leadingIcon: const Icon(Icons.format_list_numbered, size: 18),
        menuChildren: [
          MenuItemButton(
            leadingIcon: Icon(
              d.autoManaged ? Icons.check_box : Icons.check_box_outline_blank,
              size: 18,
            ),
            onPressed: () => _act(
              l10n.actQueue,
              notifier.setAutoManaged(d.infohash, !d.autoManaged),
            ),
            child: Text(l10n.autoManaged),
          ),
          const Divider(height: 1),
          item(
            Icons.vertical_align_top,
            l10n.queueTop,
            () => _act(
              l10n.actQueue,
              notifier.moveInQueue(d.infohash, QueueOp.top),
            ),
          ),
          item(
            Icons.keyboard_arrow_up,
            l10n.queueUp,
            () => _act(
              l10n.actQueue,
              notifier.moveInQueue(d.infohash, QueueOp.up),
            ),
          ),
          item(
            Icons.keyboard_arrow_down,
            l10n.queueDown,
            () => _act(
              l10n.actQueue,
              notifier.moveInQueue(d.infohash, QueueOp.down),
            ),
          ),
          item(
            Icons.vertical_align_bottom,
            l10n.queueBottom,
            () => _act(
              l10n.actQueue,
              notifier.moveInQueue(d.infohash, QueueOp.bottom),
            ),
          ),
        ],
        child: Text(
          d.queuePosition >= 0
              ? l10n.queueMenuWithPos(d.queuePosition)
              : l10n.queueMenu,
        ),
      ),
      SubmenuButton(
        leadingIcon: const Icon(Icons.shield_outlined, size: 18),
        menuChildren: [
          for (final h in const [0, 1, 2, 3])
            MenuItemButton(
              leadingIcon: Icon(
                d.hops == h ? Icons.check : Icons.shield_outlined,
                size: 18,
              ),
              onPressed: () =>
                  _act(l10n.actAnon, notifier.setAnonHops(d.infohash, h)),
              child: Text(h == 0 ? l10n.ctxDirectHop : l10n.ctxHops(h)),
            ),
        ],
        child: Text(
          l10n.anonMenuTitle(
            d.hops == 0 ? l10n.anonDirect : l10n.ctxHops(d.hops),
          ),
        ),
      ),
      const Divider(height: 1),
      SubmenuButton(
        leadingIcon: const Icon(Icons.speed, size: 18),
        menuChildren: [
          _rateLimitSubmenu(
            label: l10n.downloadDir,
            current: d.downloadLimit,
            presets: _kRatePresets,
            onPick: (v) => _act(
              l10n.actDownLimit,
              notifier.setRateLimits(d.infohash, downloadLimit: v),
            ),
            onCustom: () => showRateLimitsDialog(context, d),
          ),
          _rateLimitSubmenu(
            label: l10n.uploadDir,
            current: d.uploadLimit,
            presets: _kRatePresets,
            onPick: (v) => _act(
              l10n.actUpLimit,
              notifier.setRateLimits(d.infohash, uploadLimit: v),
            ),
            onCustom: () => showRateLimitsDialog(context, d),
          ),
        ],
        child: Text(l10n.rateLimits),
      ),
      item(
        Icons.balance,
        l10n.seedRatioDialog,
        () => showSeedingRatioDialog(context, d),
      ),
      item(
        Icons.fact_check_outlined,
        l10n.recheck,
        () => _act(l10n.actRecheck, notifier.recheck(d.infohash)),
      ),
      item(
        Icons.drive_file_move_outlined,
        l10n.moveFolder,
        () => showMoveStorageDialog(context, d),
      ),
      const Divider(height: 1),
      if (d.isPrivate)
        item(
          Icons.public,
          l10n.republishAnon,
          () => _act(
            l10n.actRepublish,
            notifier.clonePublic(d.infohash).then((ih) {
              if (ih.isNotEmpty) {
                _toast(l10n.twinCreated(ih));
              }
            }),
          ),
        ),
      item(Icons.link, l10n.ctxCopyMagnet, () {
        Clipboard.setData(ClipboardData(text: magnetUri));
        _toast(l10n.toastMagnetCopied);
      }),
      item(Icons.copy, l10n.ctxCopyInfohash, () {
        Clipboard.setData(ClipboardData(text: d.infohash));
        _toast(l10n.toastInfohashCopied);
      }),
      const Divider(height: 1),
      MenuItemButton(
        leadingIcon: Icon(
          Icons.delete_outline,
          size: 18,
          color: Theme.of(context).colorScheme.error,
        ),
        // Meme dialogue de confirmation que la poubelle de la barre
        // d'actions (option « supprimer aussi les donnees ») — un
        // menu contextuel ne doit pas supprimer sans garde-fou.
        onPressed: () => confirmRemoveSelected(context, ref, {d.infohash}),
        child: Text(
          l10n.delete,
          style: TextStyle(color: Theme.of(context).colorScheme.error),
        ),
      ),
    ];
  }
}

/// Liste compacte : ListTiles + détail en bottom sheet au tap.
class _CompactList extends ConsumerWidget {
  const _CompactList({required this.downloads});

  final List<Download> downloads;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return ListView.builder(
      itemCount: downloads.length,
      itemBuilder: (context, i) {
        final d = downloads[i];
        return ListTile(
          leading: AnonBadge(download: d),
          title: Text(
            d.name.isEmpty ? d.infohash : d.name,
            overflow: TextOverflow.ellipsis,
          ),
          subtitle: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              DownloadProgressBar(download: d),
              const SizedBox(height: 2),
              Text(
                '↓${context.fmtRate(d.speedDown)}'
                ' · ↑${context.fmtRate(d.speedUp)}',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          ),
          onLongPress: () {
            final box = context.findRenderObject() as RenderBox?;
            final pos = box != null
                ? box.localToGlobal(Offset.zero)
                : Offset.zero;
            _DownloadsContextMenu.show(context, pos + const Offset(50, 50), d);
          },
          onTap: () => showModalBottomSheet(
            context: context,
            isScrollControlled: true,
            showDragHandle: true,
            builder: (_) => SizedBox(
              height: MediaQuery.sizeOf(context).height * 0.7,
              child: DownloadDetailPanel(download: d),
            ),
          ),
        );
      },
    );
  }
}

/// Bascule table / grille / liste compacte — desktop uniquement
/// (le mode choisi est persisté via `downloadViewModeProvider`).
class _ViewModeSelector extends ConsumerWidget {
  const _ViewModeSelector({required this.mode});

  final DownloadViewMode mode;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return SegmentedButton<DownloadViewMode>(
      showSelectedIcon: false,
      style: const ButtonStyle(
        visualDensity: VisualDensity.compact,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      ),
      segments: [
        ButtonSegment(
          value: DownloadViewMode.table,
          icon: const Icon(Icons.table_rows_outlined, size: 18),
          tooltip: context.l10n.viewTable,
        ),
        ButtonSegment(
          value: DownloadViewMode.grid,
          icon: const Icon(Icons.grid_view_outlined, size: 18),
          tooltip: context.l10n.viewGrid,
        ),
        ButtonSegment(
          value: DownloadViewMode.compact,
          icon: const Icon(Icons.view_list_outlined, size: 18),
          tooltip: context.l10n.viewCompact,
        ),
      ],
      selected: {mode},
      onSelectionChanged: (s) =>
          ref.read(downloadViewModeProvider.notifier).set(s.first),
    );
  }
}

/// Vue grille desktop : cartes ~340 px reprenant les informations
/// essentielles de la ligne (nom, santé, badges, progression, état,
/// débits). Mêmes interactions que la table : clic = sélection
/// (Ctrl/Shift), clic droit = menu contextuel.
class _GridView extends ConsumerWidget {
  const _GridView({required this.downloads});

  final List<Download> downloads;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return GridView.builder(
      padding: const EdgeInsets.all(AppSpacing.sm),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 340,
        mainAxisExtent: 120,
        mainAxisSpacing: AppSpacing.sm,
        crossAxisSpacing: AppSpacing.sm,
      ),
      itemCount: downloads.length,
      itemBuilder: (context, i) =>
          _GridCard(download: downloads[i], ordered: downloads),
    );
  }
}

class _GridCard extends ConsumerWidget {
  const _GridCard({required this.download, required this.ordered});

  final Download download;
  final List<Download> ordered;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final d = download;
    final selection = ref.watch(downloadSelectionProvider);
    final sel = ref.read(downloadSelectionProvider.notifier);
    final selected = selection.contains(d.infohash);
    final scheme = Theme.of(context).colorScheme;
    final small = Theme.of(context).textTheme.bodySmall;

    return Card(
      clipBehavior: Clip.antiAlias,
      color: selected ? scheme.secondaryContainer : null,
      margin: EdgeInsets.zero,
      child: InkWell(
        onTap: () {
          final kb = HardwareKeyboard.instance;
          sel.click(
            d.infohash,
            ordered,
            ctrl: kb.isControlPressed || kb.isMetaPressed,
            shift: kb.isShiftPressed,
          );
        },
        onSecondaryTapUp: (details) =>
            _DownloadsContextMenu.show(context, details.globalPosition, d),
        child: Padding(
          padding: const EdgeInsets.all(AppSpacing.sm),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  _HealthDot(download: d),
                  const SizedBox(width: AppSpacing.xs),
                  Expanded(
                    child: Text(
                      d.name.isEmpty ? d.infohash : d.name,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  _RowBadges(download: d),
                ],
              ),
              const SizedBox(height: AppSpacing.xs),
              DownloadProgressBar(download: d),
              const Spacer(),
              Text(
                '↓${context.fmtRate(d.speedDown)}'
                ' · ↑${context.fmtRate(d.speedUp)}',
                style: small,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

