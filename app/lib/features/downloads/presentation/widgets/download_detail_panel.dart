// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/platform/open_url.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/download.dart';
import '../../domain/download_tracker.dart';
import '../providers/downloads_providers.dart';
import 'download_actions.dart';

/// Panneau de détail sous la liste (onglets Détails/Fichiers/Trackers/
/// Pairs — comme la GUI Tribler).
class DownloadDetailPanel extends ConsumerWidget {
  const DownloadDetailPanel({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    return DefaultTabController(
      length: 4,
      child: Column(
        children: [
          TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: l10n.tabDetails),
              Tab(text: l10n.tabFiles),
              const Tab(text: 'Trackers'),
              Tab(text: l10n.tabPeers),
            ],
          ),
          Expanded(
            child: TabBarView(
              children: [
                _DetailsTab(download: download),
                _FilesTab(download: download),
                _TrackersTab(download: download),
                _PeersTab(download: download),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _DetailsTab extends ConsumerStatefulWidget {
  const _DetailsTab({required this.download});

  final Download download;

  @override
  ConsumerState<_DetailsTab> createState() => _DetailsTabState();
}

class _DetailsTabState extends ConsumerState<_DetailsTab> {
  /// Historique glissant des débits (un échantillon par poll de
  /// `downloadsProvider`) — alimente le sparkline. Mémoire bornée.
  static const _maxSamples = 120;
  final _history = <({int down, int up})>[];

  Download get download => widget.download;

  @override
  void initState() {
    super.initState();
    _record(widget.download);
  }

  @override
  void didUpdateWidget(_DetailsTab oldWidget) {
    super.didUpdateWidget(oldWidget);
    _record(widget.download);
  }

  void _record(Download d) {
    if (identical(d, _history.isEmpty ? null : _lastDownload)) return;
    _lastDownload = d;
    _history.add((down: d.speedDown, up: d.speedUp));
    if (_history.length > _maxSamples) {
      _history.removeRange(0, _history.length - _maxSamples);
    }
  }

  Download? _lastDownload;

  String _date(int epoch) {
    if (epoch <= 0) return '—';
    final dt = DateTime.fromMillisecondsSinceEpoch(epoch * 1000);
    String two(int v) => v.toString().padLeft(2, '0');
    return '${dt.year}-${two(dt.month)}-${two(dt.day)} '
        '${two(dt.hour)}:${two(dt.minute)}';
  }

  @override
  Widget build(BuildContext context) {
    final d = download;
    final l10n = context.l10n;
    final notifier = ref.read(downloadsProvider.notifier);
    final magnetUri =
        'magnet:?xt=urn:btih:${d.infohash}&dn=${Uri.encodeComponent(d.name.isEmpty ? d.infohash : d.name)}';

    // Scrollbar persistante : le contenu « Détails » dépasse la
    // hauteur du panneau — le pouce rend le défilement visible.
    return Scrollbar(
      thumbVisibility: true,
      child: ListView(
        padding: const EdgeInsets.all(AppSpacing.md),
        children: [
          SpeedSparkline(history: _history),
          const SizedBox(height: AppSpacing.sm),
          LinearProgressIndicator(value: d.progress.clamp(0.0, 1.0)),
          const SizedBox(height: AppSpacing.md),
          Wrap(
            spacing: AppSpacing.sm,
            runSpacing: AppSpacing.xs,
            children: [
              // « Ouvrir le dossier » : explorateur natif — sans objet
              // sur web (le chemin appartient à la machine du daemon).
              if (!kIsWeb && d.destination.isNotEmpty)
                OutlinedButton.icon(
                  icon: const Icon(Icons.folder_open, size: 16),
                  label: Text(l10n.openFolder),
                  onPressed: () => openPath(d.destination),
                ),
              OutlinedButton.icon(
                icon: const Icon(Icons.link, size: 16),
                label: Text(l10n.ctxCopyMagnet),
                onPressed: () {
                  Clipboard.setData(ClipboardData(text: magnetUri));
                  ScaffoldMessenger.of(
                    context,
                  ).showSnackBar(SnackBar(content: Text(l10n.copyMagnetToast)));
                },
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.copy, size: 16),
                label: Text(l10n.ctxCopyInfohash),
                onPressed: () {
                  Clipboard.setData(ClipboardData(text: d.infohash));
                  ScaffoldMessenger.of(context).showSnackBar(
                    SnackBar(content: Text(l10n.copyInfohashToast)),
                  );
                },
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.speed, size: 16),
                label: Text(l10n.limitsBtn),
                onPressed: () => showRateLimitsDialog(context, d),
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.balance, size: 16),
                label: Text(l10n.ratioSeedBtn),
                onPressed: () => showSeedingRatioDialog(context, d),
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.drive_file_move_outlined, size: 16),
                label: Text(l10n.moveBtn),
                onPressed: () => showMoveStorageDialog(context, d),
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.fact_check_outlined, size: 16),
                label: Text(l10n.recheckBtn),
                onPressed: () async {
                  try {
                    await notifier.recheck(d.infohash);
                  } catch (e) {
                    if (context.mounted) {
                      showDownloadError(context, l10n.actRecheck, e);
                    }
                  }
                },
              ),
            ],
          ),
          const SizedBox(height: AppSpacing.md),
          _row(context, l10n.rowName, d.name.isEmpty ? l10n.noName : d.name),
          _row(context, l10n.rowStatus, d.status),
          _row(context, l10n.rowSize, context.fmtBytes(d.size)),
          _row(
            context,
            l10n.rowHealth,
            l10n.healthValue(d.numSeeds, d.numPeers),
          ),
          _row(
            context,
            l10n.rowAnon,
            d.anonDownload
                ? l10n.hopsValue(
                    d.hops,
                    d.safeSeeding ? l10n.safeSeedingSuffix : '',
                  )
                : l10n.anonDirect,
          ),
          _row(
            context,
            l10n.rowDest,
            d.destination.isEmpty ? l10n.daemonDefault : d.destination,
          ),
          _row(
            context,
            l10n.rowQueue,
            (d.autoManaged ? l10n.queueAuto : l10n.queueManual) +
                (d.queuePosition >= 0
                    ? l10n.queuePosSuffix(d.queuePosition)
                    : ''),
          ),
          _row(context, l10n.rowLimits, formatLimits(d, context.uiLang)),
          _row(
            context,
            l10n.rowTotalTraffic,
            '↑ ${context.fmtBytes(d.uploaded)} · ↓ ${context.fmtBytes(d.downloaded)}',
          ),
          _row(
            context,
            l10n.rowSeedRatio,
            d.seedingRatio > 0 ? d.seedingRatio.toString() : l10n.defaultValue,
          ),
          _row(context, l10n.rowAdded, _date(d.timeAdded)),
          _row(context, l10n.rowFinished, _date(d.timeFinished)),
          if (d.error.isNotEmpty) _row(context, l10n.rowError, d.error),
          const SizedBox(height: AppSpacing.sm),
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  d.infohash,
                  style: Theme.of(context).textTheme.bodySmall
                      ?.copyWith(fontFamily: 'monospace'),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _row(BuildContext context, String label, String value) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 110,
            child: Text(
              label,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          Expanded(child: Text(value)),
        ],
      ),
    );
  }
}

/// Sparkline des débits ↓/↑ — `CustomPainter` maison (pas de
/// dépendance chart pour un graphe de 120 points).
class SpeedSparkline extends StatelessWidget {
  const SpeedSparkline({super.key, required this.history});

  /// Échantillons chronologiques (un par poll).
  final List<({int down, int up})> history;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final small = Theme.of(context).textTheme.labelSmall;
    final peak = history.fold(
      1,
      (m, s) => s.down > m ? s.down : (s.up > m ? s.up : m),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(Icons.arrow_downward, size: 12, color: scheme.primary),
            Text(
              ' ${context.fmtRate(history.lastOrNull?.down ?? 0)}',
              style: small,
            ),
            const SizedBox(width: AppSpacing.sm),
            Icon(Icons.arrow_upward, size: 12, color: scheme.tertiary),
            Text(
              ' ${context.fmtRate(history.lastOrNull?.up ?? 0)}',
              style: small,
            ),
            const Spacer(),
            Text(context.l10n.peakLabel(context.fmtRate(peak)), style: small),
          ],
        ),
        const SizedBox(height: 4),
        SizedBox(
          height: 48,
          width: double.infinity,
          child: CustomPaint(
            painter: _SparklinePainter(
              history: history,
              downColor: scheme.primary,
              upColor: scheme.tertiary,
              gridColor: scheme.outlineVariant,
            ),
          ),
        ),
      ],
    );
  }
}

class _SparklinePainter extends CustomPainter {
  _SparklinePainter({
    required this.history,
    required this.downColor,
    required this.upColor,
    required this.gridColor,
  });

  final List<({int down, int up})> history;
  final Color downColor;
  final Color upColor;
  final Color gridColor;

  @override
  void paint(Canvas canvas, Size size) {
    final grid = Paint()
      ..color = gridColor
      ..strokeWidth = 1;
    canvas.drawLine(
      Offset(0, size.height - 0.5),
      Offset(size.width, size.height - 0.5),
      grid,
    );
    if (history.length < 2) return;

    var peak = 1.0;
    for (final s in history) {
      if (s.down > peak) peak = s.down.toDouble();
      if (s.up > peak) peak = s.up.toDouble();
    }
    final dx = size.width / (history.length - 1);

    void draw(Color color, int Function(({int down, int up}) s) pick) {
      final paint = Paint()
        ..color = color
        ..strokeWidth = 1.5
        ..style = PaintingStyle.stroke
        ..strokeJoin = StrokeJoin.round;
      final path = Path();
      for (var i = 0; i < history.length; i++) {
        final y =
            size.height - (pick(history[i]) / peak) * (size.height - 4) - 2;
        i == 0 ? path.moveTo(0, y) : path.lineTo(i * dx, y);
      }
      canvas.drawPath(path, paint);
    }

    draw(downColor, (s) => s.down);
    draw(upColor, (s) => s.up);
  }

  @override
  bool shouldRepaint(_SparklinePainter old) => !identical(old.history, history);
}

class _FilesTab extends ConsumerWidget {
  const _FilesTab({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final files = ref.watch(downloadFilesProvider(download.infohash));
    return files.when(
      loading: () => const Center(child: CircularProgressIndicator()),
      error: (e, _) => ErrorState(
        error: e,
        onRetry: () => ref.invalidate(downloadFilesProvider(download.infohash)),
      ),
      data: (fileList) => fileList.isEmpty
          ? EmptyState(
              icon: Icons.folder_open,
              title: context.l10n.noFiles,
              message: context.l10n.noFilesMeta,
            )
          : ListView.builder(
              padding: const EdgeInsets.symmetric(horizontal: AppSpacing.md),
              itemCount: fileList.length,
              itemBuilder: (context, i) {
                final f = fileList[i];
                final filePath = download.destination.isNotEmpty
                    ? '${download.destination}/${f.name}'
                    : f.name;
                return ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: Tooltip(
                    message: context.l10n.includeFile,
                    child: Checkbox(
                      value: f.included,
                      onChanged: (v) async {
                        try {
                          await ref
                              .read(downloadsProvider.notifier)
                              .setFileIncluded(
                                download.infohash,
                                f.index,
                                v ?? true,
                                fileList,
                              );
                        } catch (e) {
                          if (context.mounted) {
                            showDownloadError(
                              context,
                              context.l10n.actSelect,
                              e,
                            );
                          }
                        }
                      },
                    ),
                  ),
                  title: Text(f.name, overflow: TextOverflow.ellipsis),
                  subtitle: LinearProgressIndicator(value: f.fraction),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        '${(f.fraction * 100).toStringAsFixed(0)} % · '
                        '${context.fmtBytes(f.size)}',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                      PopupMenuButton<int>(
                        tooltip: context.l10n.filePriority,
                        icon: const Icon(Icons.low_priority, size: 18),
                        onSelected: (p) async {
                          try {
                            await ref
                                .read(downloadsProvider.notifier)
                                .setFilePriority(download.infohash, f.index, p);
                            if (context.mounted) {
                              ScaffoldMessenger.of(context).showSnackBar(
                                SnackBar(
                                  content: Text(
                                    context.l10n.priorityApplied(p),
                                  ),
                                ),
                              );
                            }
                          } catch (e) {
                            if (context.mounted) {
                              showDownloadError(
                                context,
                                context.l10n.actPriority,
                                e,
                              );
                            }
                          }
                        },
                        itemBuilder: (ctx) => [
                          PopupMenuItem(
                            value: 0,
                            child: Text(ctx.l10n.prioNoDownload),
                          ),
                          PopupMenuItem(
                            value: 1,
                            child: Text(ctx.l10n.prioLow),
                          ),
                          PopupMenuItem(
                            value: 4,
                            child: Text(ctx.l10n.prioNormal),
                          ),
                          PopupMenuItem(
                            value: 7,
                            child: Text(ctx.l10n.prioHigh),
                          ),
                        ],
                      ),
                      if (!kIsWeb && download.destination.isNotEmpty)
                        IconButton(
                          icon: const Icon(Icons.folder_open, size: 18),
                          tooltip: context.l10n.openLocation,
                          onPressed: () => openPath(filePath),
                        ),
                      if (kIsWeb)
                        // Streaming : le navigateur lit directement
                        // `/api/downloads/{ih}/stream/{i}` (player
                        // HTML5) — `?key=` car un onglet ne peut pas
                        // poser `X-Api-Key` (accepté par auth.rs).
                        IconButton(
                          icon: const Icon(Icons.play_circle_outline, size: 18),
                          tooltip: context.l10n.streamInTab,
                          onPressed: () {
                            final cfg = ref.read(appConfigProvider);
                            openExternalUrl(
                              '${cfg.baseUrl}/api/downloads/${download.infohash}'
                              '/stream/${f.index}?key=${cfg.apiKey}',
                            );
                          },
                        ),
                    ],
                  ),
                );
              },
            ),
    );
  }
}

class _TrackersTab extends ConsumerWidget {
  const _TrackersTab({required this.download});

  final Download download;

  Future<void> _addTrackerDialog(BuildContext context, WidgetRef ref) async {
    final controller = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(ctx.l10n.addTracker),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: InputDecoration(
            labelText: ctx.l10n.trackerUrl,
            hintText: 'udp://tracker.example.com:6969/announce',
            prefixIcon: const Icon(Icons.track_changes),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(ctx.l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(ctx.l10n.add),
          ),
        ],
      ),
    );
    if (ok == true && controller.text.trim().isNotEmpty) {
      final url = controller.text.trim();
      try {
        await ref
            .read(downloadsProvider.notifier)
            .addTracker(download.infohash, url);
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text(context.l10n.trackerAdded(url))),
          );
        }
      } catch (e) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text(context.l10n.trackerAddError('$e'))),
          );
        }
      }
    }
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final trackersAsync = ref.watch(
      downloadTrackersProvider(download.infohash),
    );

    return trackersAsync.when(
      loading: () => const Center(child: CircularProgressIndicator()),
      error: (_, _) => _buildTrackersList(context, ref, download.trackers),
      data: (trackers) => _buildTrackersList(
        context,
        ref,
        trackers.isNotEmpty ? trackers : download.trackers,
      ),
    );
  }

  Future<void> _runTrackerAction(
    BuildContext context,
    WidgetRef ref,
    String label,
    Future<void> Function() action,
  ) async {
    try {
      await action();
      if (context.mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(context.l10n.actionDone(label))));
      }
    } catch (e) {
      if (context.mounted) showDownloadError(context, label, e);
    }
  }

  /// Les pseudo-entrées `[DHT]`/`[PeX]` ne sont pas de vrais trackers
  /// (`trackers_json` les ajoute pour l'affichage, comme Python).
  bool _isPseudoTracker(DownloadTracker t) => t.url.startsWith('[');

  Widget _buildTrackersList(
    BuildContext context,
    WidgetRef ref,
    List<DownloadTracker> trackers,
  ) {
    final l10n = context.l10n;
    final notifier = ref.read(downloadsProvider.notifier);
    final ih = download.infohash;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.xs,
          ),
          child: Row(
            children: [
              Text(
                l10n.trackerCount(trackers.length),
                style: Theme.of(context).textTheme.titleSmall,
              ),
              const Spacer(),
              TextButton.icon(
                icon: const Icon(Icons.playlist_add, size: 16),
                label: Text(l10n.defaultTrackers),
                onPressed: () => _runTrackerAction(
                  context,
                  ref,
                  l10n.actDefaultTrackers,
                  () => notifier.addDefaultTrackers(ih),
                ),
              ),
              const SizedBox(width: AppSpacing.xs),
              FilledButton.tonalIcon(
                icon: const Icon(Icons.add, size: 16),
                label: Text(l10n.addTracker),
                onPressed: () => _addTrackerDialog(context, ref),
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(
          child: trackers.isEmpty
              ? EmptyState(
                  icon: Icons.track_changes,
                  title: l10n.noTrackers,
                  message: l10n.noTrackersHint,
                  action: FilledButton.icon(
                    icon: const Icon(Icons.add),
                    label: Text(l10n.addTracker),
                    onPressed: () => _addTrackerDialog(context, ref),
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.all(AppSpacing.md),
                  itemCount: trackers.length,
                  itemBuilder: (context, i) {
                    final t = trackers[i];
                    final pseudo = _isPseudoTracker(t);
                    return ListTile(
                      dense: true,
                      leading: const Icon(Icons.sensors, size: 20),
                      title: SelectableText(t.url),
                      subtitle: Text(
                        context.l10n.trackerStatus(t.status),
                        style: TextStyle(
                          color: t.status.toLowerCase().contains('error')
                              ? Colors.red
                              : t.status == 'Working'
                              ? Colors.green
                              : null,
                        ),
                      ),
                      // `-1` = tracker pas encore scrapé (convention
                      // `TrackerStatusDict` Python).
                      trailing: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            t.peers < 0
                                ? '—'
                                : 'P ${t.peers} · S ${t.seeds} · L ${t.leeches}',
                          ),
                          if (!pseudo) ...[
                            IconButton(
                              icon: const Icon(Icons.campaign, size: 18),
                              tooltip: context.l10n.forceAnnounce,
                              onPressed: () => _runTrackerAction(
                                context,
                                ref,
                                context.l10n.actAnnounce,
                                () => notifier.forceTrackerAnnounce(ih, t.url),
                              ),
                            ),
                            IconButton(
                              icon: const Icon(
                                Icons.remove_circle_outline,
                                size: 18,
                              ),
                              tooltip: context.l10n.removeTracker,
                              onPressed: () => _runTrackerAction(
                                context,
                                ref,
                                context.l10n.actRemoveTracker,
                                () => notifier.removeTracker(ih, t.url),
                              ),
                            ),
                          ],
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _PeersTab extends StatelessWidget {
  const _PeersTab({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final d = download;
    final theme = Theme.of(context);

    return ListView(
      padding: const EdgeInsets.all(AppSpacing.md),
      children: [
        Card(
          child: Padding(
            padding: const EdgeInsets.all(AppSpacing.md),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  context.l10n.swarmStats,
                  style: theme.textTheme.titleMedium,
                ),
                const SizedBox(height: AppSpacing.sm),
                _statRow(context.l10n.seedersConn, '${d.numSeeds}'),
                _statRow(context.l10n.leechersConn, '${d.numPeers}'),
                _statRow(context.l10n.totalPeersConn, '${d.numConnectedPeers}'),
                _statRow(
                  context.l10n.downRateRow,
                  context.fmtRate(d.speedDown),
                ),
                _statRow(context.l10n.upRateRow, context.fmtRate(d.speedUp)),
                _statRow(
                  context.l10n.netMode,
                  d.anonDownload
                      ? context.l10n.netModeAnon(d.hops)
                      : context.l10n.netModeDirect,
                ),
              ],
            ),
          ),
        ),
        if (d.peers.isNotEmpty) ...[
          const SizedBox(height: AppSpacing.md),
          Text(
            context.l10n.peersConnCount(d.peers.length),
            style: theme.textTheme.titleMedium,
          ),
          const SizedBox(height: AppSpacing.sm),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: DataTable(
              headingRowHeight: 32,
              dataRowMinHeight: 32,
              dataRowMaxHeight: 36,
              columnSpacing: AppSpacing.md,
              columns: [
                DataColumn(label: Text(context.l10n.colAddress)),
                DataColumn(label: Text(context.l10n.colClient)),
                DataColumn(label: Text(context.l10n.colDir)),
                DataColumn(
                  label: Text(context.l10n.colDownRate),
                  numeric: true,
                ),
                DataColumn(label: Text(context.l10n.colUpRate), numeric: true),
                DataColumn(
                  label: Text(context.l10n.colDownTotal),
                  numeric: true,
                ),
                DataColumn(label: Text(context.l10n.colUpTotal), numeric: true),
                DataColumn(label: Text(context.l10n.colTransport)),
              ],
              rows: [
                for (final p in d.peers)
                  DataRow(
                    cells: [
                      DataCell(SelectableText('${p.ip}:${p.port}')),
                      DataCell(
                        Text(
                          p.extendedVersion.isEmpty ? '—' : p.extendedVersion,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      DataCell(
                        Tooltip(
                          message: p.direction == 'L'
                              ? context.l10n.peerDirIn
                              : context.l10n.peerDirOut,
                          child: Icon(
                            p.direction == 'L'
                                ? Icons.south_west
                                : Icons.north_east,
                            size: 16,
                          ),
                        ),
                      ),
                      DataCell(Text(context.fmtRate(p.downrate))),
                      DataCell(Text(context.fmtRate(p.uprate))),
                      DataCell(Text(context.fmtBytes(p.dtotal))),
                      DataCell(Text(context.fmtBytes(p.utotal))),
                      DataCell(
                        Text(p.connectionType.isEmpty ? '—' : p.connectionType),
                      ),
                    ],
                  ),
              ],
            ),
          ),
        ] else if (d.isActive) ...[
          const SizedBox(height: AppSpacing.md),
          EmptyState(
            icon: Icons.people_outline,
            title: context.l10n.noPeers,
            message: context.l10n.noPeersHint,
          ),
        ],
      ],
    );
  }

  Widget _statRow(String label, String value) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text(label),
          Text(value, style: const TextStyle(fontWeight: FontWeight.bold)),
        ],
      ),
    );
  }
}
