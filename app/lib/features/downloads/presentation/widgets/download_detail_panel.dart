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

class _DetailsTab extends StatelessWidget {
  const _DetailsTab({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final d = download;
    final magnetUri =
        'magnet:?xt=urn:btih:${d.infohash}&dn=${Uri.encodeComponent(d.name.isEmpty ? d.infohash : d.name)}';

    return ListView(
      padding: const EdgeInsets.all(AppSpacing.md),
      children: [
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
                      if (download.destination.isNotEmpty) ...[
                        const SizedBox(width: AppSpacing.xs),
                        IconButton(
                          icon: const Icon(Icons.folder_open, size: 18),
                          tooltip: 'Ouvrir l\'emplacement',
                          onPressed: () => openPath(filePath),
                        ),
                      ],
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

  Widget _buildTrackersList(
    BuildContext context,
    WidgetRef ref,
    List<DownloadTracker> trackers,
  ) {
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.xs,
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                '${trackers.length} tracker(s)',
                style: Theme.of(context).textTheme.titleSmall,
              ),
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
                      trailing: Text(
                        t.peers < 0
                            ? '—'
                            : 'P ${t.peers} · S ${t.seeds} · L ${t.leeches}',
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
