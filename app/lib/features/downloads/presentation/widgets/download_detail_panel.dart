import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
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
    return DefaultTabController(
      length: 4,
      child: Column(
        children: [
          const TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: 'Détails'),
              Tab(text: 'Fichiers'),
              Tab(text: 'Trackers'),
              Tab(text: 'Pairs'),
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
    final notifier = ref.read(downloadsProvider.notifier);
    final magnetUri =
        'magnet:?xt=urn:btih:${d.infohash}&dn=${Uri.encodeComponent(d.name.isEmpty ? d.infohash : d.name)}';

    return ListView(
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
            if (d.destination.isNotEmpty)
              OutlinedButton.icon(
                icon: const Icon(Icons.folder_open, size: 16),
                label: const Text('Ouvrir le dossier'),
                onPressed: () => openPath(d.destination),
              ),
            OutlinedButton.icon(
              icon: const Icon(Icons.link, size: 16),
              label: const Text('Copier le lien magnet'),
              onPressed: () {
                Clipboard.setData(ClipboardData(text: magnetUri));
                ScaffoldMessenger.of(context).showSnackBar(
                  const SnackBar(content: Text('Lien magnet copié dans le presse-papier')),
                );
              },
            ),
            OutlinedButton.icon(
              icon: const Icon(Icons.copy, size: 16),
              label: const Text('Copier l\'info-hash'),
              onPressed: () {
                Clipboard.setData(ClipboardData(text: d.infohash));
                ScaffoldMessenger.of(context).showSnackBar(
                  const SnackBar(content: Text('Info-hash copié dans le presse-papier')),
                );
              },
            ),
            OutlinedButton.icon(
              icon: const Icon(Icons.speed, size: 16),
              label: const Text('Limites…'),
              onPressed: () => showRateLimitsDialog(context, d),
            ),
            OutlinedButton.icon(
              icon: const Icon(Icons.balance, size: 16),
              label: const Text('Ratio seed…'),
              onPressed: () => showSeedingRatioDialog(context, d),
            ),
            OutlinedButton.icon(
              icon: const Icon(Icons.drive_file_move_outlined, size: 16),
              label: const Text('Déplacer…'),
              onPressed: () => showMoveStorageDialog(context, d),
            ),
            OutlinedButton.icon(
              icon: const Icon(Icons.fact_check_outlined, size: 16),
              label: const Text('Revérifier'),
              onPressed: () async {
                try {
                  await notifier.recheck(d.infohash);
                } catch (e) {
                  if (context.mounted) {
                    showDownloadError(context, 'revérification', e);
                  }
                }
              },
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.md),
        _row(context, 'Nom', d.name.isEmpty ? '(sans nom)' : d.name),
        _row(context, 'Statut', d.status),
        _row(context, 'Taille', ByteFormatter.format(d.size)),
        _row(context, 'Santé', '${d.numSeeds} seeders, ${d.numPeers} leechers'),
        _row(
          context,
          'Anonymat',
          d.anonDownload
              ? '${d.hops} saut(s)${d.safeSeeding ? ' + safe seeding' : ''}'
              : 'Direct',
        ),
        _row(
          context,
          'Destination',
          d.destination.isEmpty ? '(défaut daemon)' : d.destination,
        ),
        _row(
          context,
          'File d\'attente',
          d.autoManaged
              ? 'automatique${d.queuePosition >= 0 ? ' · position ${d.queuePosition}' : ''}'
              : d.queuePosition >= 0
              ? 'manuelle · position ${d.queuePosition}'
              : 'manuelle',
        ),
        _row(context, 'Limites', formatLimits(d)),
        _row(
          context,
          'Ratio de seed',
          d.seedingRatio > 0 ? d.seedingRatio.toString() : 'défaut',
        ),
        _row(context, 'Ajouté le', _date(d.timeAdded)),
        _row(context, 'Terminé le', _date(d.timeFinished)),
        if (d.error.isNotEmpty) _row(context, 'Erreur', d.error),
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
              ' ${ByteFormatter.formatRate(history.lastOrNull?.down ?? 0)}',
              style: small,
            ),
            const SizedBox(width: AppSpacing.sm),
            Icon(Icons.arrow_upward, size: 12, color: scheme.tertiary),
            Text(
              ' ${ByteFormatter.formatRate(history.lastOrNull?.up ?? 0)}',
              style: small,
            ),
            const Spacer(),
            Text('crête ${ByteFormatter.formatRate(peak)}', style: small),
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
        i == 0
            ? path.moveTo(0, y)
            : path.lineTo(i * dx, y);
      }
      canvas.drawPath(path, paint);
    }

    draw(downColor, (s) => s.down);
    draw(upColor, (s) => s.up);
  }

  @override
  bool shouldRepaint(_SparklinePainter old) =>
      !identical(old.history, history);
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
        message: '$e',
        onRetry: () => ref.invalidate(downloadFilesProvider(download.infohash)),
      ),
      data: (fileList) => fileList.isEmpty
          ? const EmptyState(
              icon: Icons.folder_open,
              title: 'Aucun fichier listé',
              message: 'Les métadonnées ne sont pas encore disponibles.',
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
                    message: 'Inclure dans le téléchargement',
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
                            showDownloadError(context, 'sélection', e);
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
                        '${ByteFormatter.format(f.size)}',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                      PopupMenuButton<int>(
                        tooltip: 'Priorité du fichier',
                        icon: const Icon(Icons.low_priority, size: 18),
                        onSelected: (p) async {
                          try {
                            await ref
                                .read(downloadsProvider.notifier)
                                .setFilePriority(
                                  download.infohash,
                                  f.index,
                                  p,
                                );
                            if (context.mounted) {
                              ScaffoldMessenger.of(context).showSnackBar(
                                SnackBar(
                                  content: Text('Priorité $p appliquée'),
                                ),
                              );
                            }
                          } catch (e) {
                            if (context.mounted) {
                              showDownloadError(context, 'priorité', e);
                            }
                          }
                        },
                        itemBuilder: (_) => const [
                          PopupMenuItem(value: 0, child: Text('Ne pas télécharger')),
                          PopupMenuItem(value: 1, child: Text('Priorité basse')),
                          PopupMenuItem(value: 4, child: Text('Normale')),
                          PopupMenuItem(value: 7, child: Text('Priorité haute')),
                        ],
                      ),
                      if (download.destination.isNotEmpty)
                        IconButton(
                          icon: const Icon(Icons.folder_open, size: 18),
                          tooltip: 'Ouvrir l\'emplacement',
                          onPressed: () => openPath(filePath),
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
        title: const Text('Ajouter un tracker'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'URL du tracker',
            hintText: 'udp://tracker.example.com:6969/announce',
            prefixIcon: Icon(Icons.track_changes),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Annuler'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Ajouter'),
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
            SnackBar(content: Text('Tracker ajouté : $url')),
          );
        }
      } catch (e) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Erreur ajout tracker : $e')),
          );
        }
      }
    }
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final trackersAsync = ref.watch(downloadTrackersProvider(download.infohash));

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
        ).showSnackBar(SnackBar(content: Text('$label effectué')));
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
                '${trackers.length} tracker(s)',
                style: Theme.of(context).textTheme.titleSmall,
              ),
              const Spacer(),
              TextButton.icon(
                icon: const Icon(Icons.playlist_add, size: 16),
                label: const Text('Trackers par défaut'),
                onPressed: () => _runTrackerAction(
                  context,
                  ref,
                  'ajout des trackers par défaut',
                  () => notifier.addDefaultTrackers(ih),
                ),
              ),
              const SizedBox(width: AppSpacing.xs),
              FilledButton.tonalIcon(
                icon: const Icon(Icons.add, size: 16),
                label: const Text('Ajouter un tracker'),
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
                  title: 'Aucun tracker actif',
                  message: 'Vous pouvez ajouter des trackers pour améliorer les sources.',
                  action: FilledButton.icon(
                    icon: const Icon(Icons.add),
                    label: const Text('Ajouter un tracker'),
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
                        'Statut : ${t.status}',
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
                              tooltip: 'Forcer une annonce',
                              onPressed: () => _runTrackerAction(
                                context,
                                ref,
                                'annonce forcée',
                                () => notifier.forceTrackerAnnounce(ih, t.url),
                              ),
                            ),
                            IconButton(
                              icon: const Icon(
                                Icons.remove_circle_outline,
                                size: 18,
                              ),
                              tooltip: 'Retirer ce tracker',
                              onPressed: () => _runTrackerAction(
                                context,
                                ref,
                                'retrait du tracker',
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
                Text('Statistiques de l\'essaim (Swarm)', style: theme.textTheme.titleMedium),
                const SizedBox(height: AppSpacing.sm),
                _statRow('Seeders connectés', '${d.numSeeds}'),
                _statRow('Leechers connectés', '${d.numPeers}'),
                _statRow('Total pairs connectés', '${d.numConnectedPeers}'),
                _statRow('Débit descendant actuel', ByteFormatter.formatRate(d.speedDown)),
                _statRow('Débit montant actuel', ByteFormatter.formatRate(d.speedUp)),
                _statRow(
                  'Mode réseau',
                  d.anonDownload
                      ? 'Tunnel anonyme IPv8 (${d.hops} saut(s))'
                      : 'Connexion directe BitTorrent',
                ),
              ],
            ),
          ),
        ),
        if (d.peers.isNotEmpty) ...[
          const SizedBox(height: AppSpacing.md),
          Text(
            '${d.peers.length} pair(s) connecté(s)',
            style: theme.textTheme.titleMedium,
          ),
          const SizedBox(height: AppSpacing.sm),
          for (final p in d.peers)
            Card(
              margin: const EdgeInsets.only(bottom: AppSpacing.xs),
              child: ListTile(
                dense: true,
                leading: Icon(
                  p.direction == 'L' ? Icons.south_west : Icons.north_east,
                  size: 18,
                ),
                title: Text('${p.ip}:${p.port}'),
                subtitle: Text(
                  [
                    if (p.extendedVersion.isNotEmpty) p.extendedVersion,
                    if (p.connectionType.isNotEmpty) p.connectionType,
                  ].join(' · '),
                ),
                trailing: Text(
                  '↓${ByteFormatter.format(p.dtotal)} '
                  '↑${ByteFormatter.format(p.utotal)}',
                ),
              ),
            ),
        ] else if (d.isActive) ...[
          const SizedBox(height: AppSpacing.md),
          const EmptyState(
            icon: Icons.people_outline,
            title: 'Aucun pair connecté',
            message: 'Le client recherche des pairs via trackers et DHT.',
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
