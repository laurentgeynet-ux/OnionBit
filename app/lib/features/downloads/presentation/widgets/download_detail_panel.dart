import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/download.dart';
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
                _FilesTab(infohash: download.infohash),
                _TrackersTab(download: download),
                const _PeersTab(),
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
    return ListView(
      padding: const EdgeInsets.all(AppSpacing.md),
      children: [
        LinearProgressIndicator(value: d.progress.clamp(0.0, 1.0)),
        const SizedBox(height: AppSpacing.md),
        _row(context, 'Nom', d.name),
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
            IconButton(
              tooltip: 'Copier l\'info-hash',
              icon: const Icon(Icons.copy, size: 18),
              onPressed: () =>
                  Clipboard.setData(ClipboardData(text: d.infohash)),
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
  const _FilesTab({required this.infohash});

  final String infohash;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final files = ref.watch(downloadFilesProvider(infohash));
    return files.when(
      loading: () => const Center(child: CircularProgressIndicator()),
      error: (e, _) => ErrorState(
        message: '$e',
        onRetry: () => ref.invalidate(downloadFilesProvider(infohash)),
      ),
      data: (files) => files.isEmpty
          ? const EmptyState(
              icon: Icons.folder_open,
              title: 'Aucun fichier listé',
              message: 'Les métadonnées ne sont pas encore disponibles.',
            )
          : ListView.builder(
              padding: const EdgeInsets.symmetric(horizontal: AppSpacing.md),
              itemCount: files.length,
              itemBuilder: (context, i) {
                final f = files[i];
                return ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  title: Text(f.name, overflow: TextOverflow.ellipsis),
                  subtitle: LinearProgressIndicator(value: f.fraction),
                  trailing: Text(
                    '${(f.fraction * 100).toStringAsFixed(0)} % · '
                    '${ByteFormatter.format(f.size)}',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                );
              },
            ),
    );
  }
}

class _TrackersTab extends StatelessWidget {
  const _TrackersTab({required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final trackers = download.trackers;
    if (trackers.isEmpty) {
      return const EmptyState(
        icon: Icons.track_changes,
        title: 'Aucun tracker exposé',
        message: 'Le champ `trackers` est vide côté API Rust pour le moment.',
      );
    }
    return ListView(
      padding: const EdgeInsets.all(AppSpacing.md),
      children: [
        for (final t in trackers)
          ListTile(
            dense: true,
            title: Text(t.url),
            subtitle: Text(t.status),
            trailing: Text('${t.peers} pairs'),
          ),
      ],
    );
  }
}

class _PeersTab extends StatelessWidget {
  const _PeersTab();

  @override
  Widget build(BuildContext context) {
    return const EmptyState(
      icon: Icons.people_outline,
      title: 'Pairs non exposés',
      message:
          'L\'API Rust accepte `get_peers` mais l\'ignore pour '
          'l\'instant (écart connu, cf. api_rest_mapping.md).',
    );
  }
}
