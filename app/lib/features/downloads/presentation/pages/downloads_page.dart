import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../../../core/layout/breakpoints.dart';
import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/utils/duration_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/download.dart';
import '../../domain/download_filter.dart';
import '../../domain/downloads_repository.dart';
import '../providers/downloads_providers.dart';
import '../widgets/add_download_dialog.dart';
import '../widgets/download_actions.dart';
import '../widgets/download_detail_panel.dart';
import '../widgets/download_status_chip.dart';

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
          child: async.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              message: '$e',
              onRetry: () => ref.read(downloadsProvider.notifier).refresh(),
            ),
            data: (all) {
              final visible = all
                  .where(widget.filter.matches)
                  .where(
                    (d) =>
                        _nameFilter.isEmpty ||
                        d.name.toLowerCase().contains(
                          _nameFilter.toLowerCase(),
                        ),
                  )
                  .toList();
              if (visible.isEmpty) {
                return EmptyState(
                  icon: Icons.download_outlined,
                  title: 'Aucun téléchargement',
                  message: widget.filter == DownloadFilter.all
                      ? 'Ajoutez un magnet ou un fichier .torrent.'
                      : 'Aucun élément dans le filtre « '
                            '${widget.filter.label} ».',
                  action: widget.filter == DownloadFilter.all
                      ? FilledButton.icon(
                          onPressed: () => AddDownloadDialog.show(context),
                          icon: const Icon(Icons.add),
                          label: const Text('Ajouter'),
                        )
                      : null,
                );
              }
              return _DownloadsContextMenu(
                child: compact
                    ? _CompactList(downloads: visible)
                    : _DesktopTable(downloads: visible, selection: selection),
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
    return [
      const Divider(height: 1),
      SizedBox(height: 280, child: DownloadDetailPanel(download: d)),
    ];
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

    Future<void> confirmRemove() async {
      var deleteFiles = false;
      final ok = await showDialog<bool>(
        context: context,
        builder: (ctx) => StatefulBuilder(
          builder: (ctx, setState) => AlertDialog(
            title: const Text('Supprimer'),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  '${selection.length} téléchargement(s) '
                  'seront retirés du daemon.',
                ),
                CheckboxListTile(
                  value: deleteFiles,
                  onChanged: (v) => setState(() => deleteFiles = v ?? false),
                  title: const Text('Supprimer aussi les données sur disque'),
                  contentPadding: EdgeInsets.zero,
                  dense: true,
                ),
              ],
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(ctx).pop(false),
                child: const Text('Annuler'),
              ),
              FilledButton(
                onPressed: () => Navigator.of(ctx).pop(true),
                child: const Text('Supprimer'),
              ),
            ],
          ),
        ),
      );
      if (ok == true) {
        await run(
          'supprimer',
          (ih) => notifier.remove(ih, deleteFiles: deleteFiles),
        );
        sel.clear();
      }
    }

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
                  Text(
                    'Téléchargements',
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                  const Spacer(),
                  if (onNameFilter != null)
                    SizedBox(
                      width: 220,
                      child: TextField(
                        decoration: const InputDecoration(
                          hintText: 'Filtrer par nom',
                          isDense: true,
                          prefixIcon: Icon(Icons.filter_list, size: 18),
                          border: OutlineInputBorder(),
                        ),
                        onChanged: onNameFilter,
                      ),
                    ),
                ] else ...[
                  Text(
                    '${selection.length} sélectionné(s)',
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                  const SizedBox(width: AppSpacing.md),
                  IconButton(
                    tooltip: 'Reprendre',
                    onPressed: () => run('reprendre', notifier.resume),
                    icon: const Icon(Icons.play_arrow),
                  ),
                  IconButton(
                    tooltip: 'Pause',
                    onPressed: () => run('pause', notifier.pause),
                    icon: const Icon(Icons.pause),
                  ),
                  IconButton(
                    tooltip: 'Supprimer',
                    onPressed: confirmRemove,
                    icon: const Icon(Icons.delete_outline),
                  ),
                  const Spacer(),
                  IconButton(
                    tooltip: 'Désélectionner',
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
                          label: Text(f.label),
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

  static const double _minWidth = 960;

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
                itemCount: downloads.length,
                itemBuilder: (context, i) =>
                    _DownloadRow(download: downloads[i]),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _HeaderRow extends StatelessWidget {
  const _HeaderRow();

  @override
  Widget build(BuildContext context) {
    final style = Theme.of(context).textTheme.labelSmall;
    Widget h(String s, {double? width, int flex = 0}) => flex > 0
        ? Expanded(
            flex: flex,
            child: Text(s, style: style),
          )
        : SizedBox(
            width: width,
            child: Text(s, style: style),
          );
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: AppSpacing.sm,
        vertical: AppSpacing.xs,
      ),
      child: Row(
        children: [
          const SizedBox(width: 36),
          h('Nom', flex: 4),
          h('Taille', width: 80),
          h('Progression', flex: 2),
          h('État', width: 160),
          h('↓', width: 90),
          h('↑', width: 90),
          h('ETA', width: 80),
          h('Pairs', width: 80),
          const SizedBox(width: 28),
        ],
      ),
    );
  }
}

class _DownloadRow extends ConsumerWidget {
  const _DownloadRow({required this.download});

  final Download download;

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
        onTap: () => sel.selectOnly(d.infohash),
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
                child: Text(ByteFormatter.format(d.size), style: small),
              ),
              Expanded(
                flex: 2,
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpacing.sm,
                  ),
                  child: LinearProgressIndicator(
                    value: d.progress.clamp(0.0, 1.0),
                  ),
                ),
              ),
              SizedBox(width: 160, child: DownloadStatusChip(download: d)),
              SizedBox(
                width: 90,
                child: Text(
                  ByteFormatter.formatRate(d.speedDown),
                  style: small,
                ),
              ),
              SizedBox(
                width: 90,
                child: Text(ByteFormatter.formatRate(d.speedUp), style: small),
              ),
              SizedBox(
                width: 80,
                child: Text(
                  DurationFormatter.formatSeconds(d.etaSeconds),
                  style: small,
                ),
              ),
              SizedBox(
                width: 80,
                child: Text(
                  '${d.numConnectedPeers} (${d.numPeers})',
                  style: small,
                ),
              ),
              SizedBox(width: 28, child: AnonBadge(download: d)),
            ],
          ),
        ),
      ),
    );
  }
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
    context
        .findAncestorStateOfType<_DownloadsContextMenuState>()
        ?._open(position, d);
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
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final d = _target;
    return MenuAnchor(
      controller: _controller,
      menuChildren: d == null ? const [] : _items(d),
      child: widget.child,
    );
  }

  List<Widget> _items(Download d) {
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
        d.isPaused ? 'Reprendre' : 'Mettre en pause',
        () => _act(
          d.isPaused ? 'reprise' : 'pause',
          d.isPaused
              ? notifier.resume(d.infohash)
              : notifier.pause(d.infohash),
        ),
      ),
      if (d.destination.isNotEmpty)
        item(
          Icons.folder_open,
          'Ouvrir le dossier',
          () => openPath(d.destination),
        ),
      const Divider(height: 1),
      SubmenuButton(
        leadingIcon: const Icon(Icons.format_list_numbered, size: 18),
        menuChildren: [
          MenuItemButton(
            leadingIcon: Icon(
              d.autoManaged
                  ? Icons.check_box
                  : Icons.check_box_outline_blank,
              size: 18,
            ),
            onPressed: () => _act(
              'file d\'attente',
              notifier.setAutoManaged(d.infohash, !d.autoManaged),
            ),
            child: const Text('Gestion automatique'),
          ),
          const Divider(height: 1),
          item(
            Icons.vertical_align_top,
            'Tout en haut',
            () => _act(
              'file d\'attente',
              notifier.moveInQueue(d.infohash, QueueOp.top),
            ),
          ),
          item(
            Icons.keyboard_arrow_up,
            'Monter',
            () => _act(
              'file d\'attente',
              notifier.moveInQueue(d.infohash, QueueOp.up),
            ),
          ),
          item(
            Icons.keyboard_arrow_down,
            'Descendre',
            () => _act(
              'file d\'attente',
              notifier.moveInQueue(d.infohash, QueueOp.down),
            ),
          ),
          item(
            Icons.vertical_align_bottom,
            'Tout en bas',
            () => _act(
              'file d\'attente',
              notifier.moveInQueue(d.infohash, QueueOp.bottom),
            ),
          ),
        ],
        child: Text(
          'File d\'attente${d.queuePosition >= 0 ? ' — position ${d.queuePosition}' : ''}',
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
                  _act('anonymat', notifier.setAnonHops(d.infohash, h)),
              child: Text(
                h == 0 ? 'Direct (0 saut)' : '$h saut${h > 1 ? 's' : ''}',
              ),
            ),
        ],
        child: Text(
          'Anonymat : ${d.hops == 0 ? 'direct' : '${d.hops} saut${d.hops > 1 ? 's' : ''}'}',
        ),
      ),
      const Divider(height: 1),
      item(
        Icons.speed,
        'Limites de débit…',
        () => showRateLimitsDialog(context, d),
      ),
      item(Icons.balance, 'Ratio de seed…', () => showSeedingRatioDialog(context, d)),
      item(
        Icons.fact_check_outlined,
        'Revérifier les données',
        () => _act('revérification', notifier.recheck(d.infohash)),
      ),
      item(
        Icons.drive_file_move_outlined,
        'Déplacer le dossier…',
        () => showMoveStorageDialog(context, d),
      ),
      const Divider(height: 1),
      item(Icons.link, 'Copier le lien magnet', () {
        Clipboard.setData(ClipboardData(text: magnetUri));
        _toast('Lien magnet copié');
      }),
      item(Icons.copy, 'Copier l\'info-hash', () {
        Clipboard.setData(ClipboardData(text: d.infohash));
        _toast('Info-hash copié');
      }),
      const Divider(height: 1),
      MenuItemButton(
        leadingIcon: Icon(
          Icons.delete_outline,
          size: 18,
          color: Theme.of(context).colorScheme.error,
        ),
        onPressed: () => _act('suppression', notifier.remove(d.infohash)),
        child: Text(
          'Supprimer',
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
              LinearProgressIndicator(value: d.progress.clamp(0, 1)),
              const SizedBox(height: 2),
              Text(
                '${d.status} · ↓${ByteFormatter.formatRate(d.speedDown)}'
                ' · ↑${ByteFormatter.formatRate(d.speedUp)}',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          ),
          trailing: DownloadStatusChip(download: d),
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
