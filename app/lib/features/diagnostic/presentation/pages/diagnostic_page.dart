import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/diagnostic_models.dart';
import '../providers/diagnostic_providers.dart';

/// Page « Diagnostic » — tout ce qui est interne au réseau vit ici
/// (overlays, circuits, relais, sorties, swarms, pairs, journaux),
/// jamais dans le parcours principal.
class DiagnosticPage extends StatelessWidget {
  const DiagnosticPage({super.key});

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 7,
      child: Column(
        children: [
          const TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: 'Overlays'),
              Tab(text: 'Circuits'),
              Tab(text: 'Relais'),
              Tab(text: 'Sorties'),
              Tab(text: 'Swarms'),
              Tab(text: 'Pairs'),
              Tab(text: 'Journaux'),
            ],
          ),
          const Expanded(
            child: TabBarView(
              children: [
                _OverlaysTab(),
                _CircuitsTab(),
                _RelaysTab(),
                _ExitsTab(),
                _SwarmsTab(),
                _PeersTab(),
                _LogsTab(),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// Scaffolding commun d'un onglet : refresh + AsyncValue → liste.
class _TabScaffold<T> extends StatelessWidget {
  const _TabScaffold({
    required this.value,
    required this.onRetry,
    required this.itemBuilder,
    required this.emptyTitle,
    this.emptyMessage,
  });

  final AsyncValue<List<T>> value;
  final VoidCallback onRetry;
  final Widget Function(T item) itemBuilder;
  final String emptyTitle;
  final String? emptyMessage;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Align(
          alignment: Alignment.centerRight,
          child: IconButton(
            tooltip: 'Rafraîchir',
            icon: const Icon(Icons.refresh, size: 18),
            onPressed: onRetry,
          ),
        ),
        Expanded(
          child: value.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(message: '$e', onRetry: onRetry),
            data: (items) => items.isEmpty
                ? EmptyState(
                    icon: Icons.insights,
                    title: emptyTitle,
                    message: emptyMessage,
                  )
                : ListView(
                    padding: const EdgeInsets.symmetric(
                      horizontal: AppSpacing.md,
                    ),
                    children: [for (final i in items) itemBuilder(i)],
                  ),
          ),
        ),
      ],
    );
  }
}

class _OverlaysTab extends ConsumerWidget {
  const _OverlaysTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<OverlayInfo>(
      value: ref.watch(overlaysProvider),
      onRetry: () => ref.invalidate(overlaysProvider),
      emptyTitle: 'Aucun overlay',
      emptyMessage: 'La stack IPv8 est-elle active ?',
      itemBuilder: (o) => ListTile(
        dense: true,
        leading: const Icon(Icons.hub_outlined, size: 20),
        title: Text(o.name),
        subtitle: Text(o.id, overflow: TextOverflow.ellipsis),
        trailing: Text('${o.peers} pairs'),
      ),
    );
  }
}

class _CircuitsTab extends ConsumerWidget {
  const _CircuitsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<CircuitInfo>(
      value: ref.watch(tunnelCircuitsProvider),
      onRetry: () => ref.invalidate(tunnelCircuitsProvider),
      emptyTitle: 'Aucun circuit',
      emptyMessage:
          'Les circuits anonymes sont construits quand un '
          'téléchargement en demande.',
      itemBuilder: (c) => ListTile(
        dense: true,
        leading: Icon(
          c.ready ? Icons.link : Icons.link_off,
          size: 20,
          color: c.ready ? Colors.green : Theme.of(context).colorScheme.outline,
        ),
        title: Text('#${c.id} · ${c.type}'),
        subtitle: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '${c.actualHops}/${c.goalHops} sauts · ${c.state}'
              '${c.infoHash != null ? ' · ${c.infoHash}' : ''}',
            ),
            // Route réellement prise : mid hex de chaque saut, dans
            // l'ordre (+ saut en cours d'ajout suffixé « … »).
            if (c.verifiedHops.isNotEmpty)
              Text(
                'route : ${[
                  for (final h in c.verifiedHops)
                    h.length > 8 ? h.substring(0, 8) : h,
                  if (c.unverifiedHop.isNotEmpty)
                    '${c.unverifiedHop.substring(0, c.unverifiedHop.length.clamp(0, 8))}…',
                ].join(' → ')}',
                style: const TextStyle(
                  fontFamily: 'monospace',
                  fontSize: 11,
                ),
              ),
          ],
        ),
        trailing: Text(
          '↑${ByteFormatter.format(c.bytesUp)} '
          '↓${ByteFormatter.format(c.bytesDown)}',
        ),
      ),
    );
  }
}

class _RelaysTab extends ConsumerWidget {
  const _RelaysTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<RelayInfo>(
      value: ref.watch(tunnelRelaysProvider),
      onRetry: () => ref.invalidate(tunnelRelaysProvider),
      emptyTitle: 'Aucun relais',
      itemBuilder: (r) => ListTile(
        dense: true,
        title: Text('${r.circuitIn} → ${r.circuitOut}'),
        subtitle: Text(r.rendezvous ? 'relais de rendez-vous' : 'relais'),
        trailing: Text(
          '↑${ByteFormatter.format(r.bytesUp)} '
          '↓${ByteFormatter.format(r.bytesDown)}',
        ),
      ),
    );
  }
}

class _ExitsTab extends ConsumerWidget {
  const _ExitsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<ExitInfo>(
      value: ref.watch(tunnelExitsProvider),
      onRetry: () => ref.invalidate(tunnelExitsProvider),
      emptyTitle: 'Aucune sortie',
      itemBuilder: (e) => ListTile(
        dense: true,
        leading: Icon(e.enabled ? Icons.exit_to_app : Icons.block, size: 20),
        title: Text('circuit #${e.circuitId}'),
        trailing: Text(e.enabled ? 'active' : 'inactive'),
      ),
    );
  }
}

class _SwarmsTab extends ConsumerWidget {
  const _SwarmsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<SwarmInfo>(
      value: ref.watch(tunnelSwarmsProvider),
      onRetry: () => ref.invalidate(tunnelSwarmsProvider),
      emptyTitle: 'Aucun swarm caché',
      emptyMessage: 'Apparaît quand un téléchargement anonyme démarre.',
      itemBuilder: (s) => ListTile(
        dense: true,
        leading: Icon(s.seeder ? Icons.upload : Icons.download, size: 20),
        title: Text(s.infoHash, overflow: TextOverflow.ellipsis),
        trailing: Text('${s.connections} connexion(s)'),
      ),
    );
  }
}

class _PeersTab extends ConsumerWidget {
  const _PeersTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<TunnelPeerInfo>(
      value: ref.watch(tunnelPeersProvider),
      onRetry: () => ref.invalidate(tunnelPeersProvider),
      emptyTitle: 'Aucun pair tunnel',
      itemBuilder: (p) => ListTile(
        dense: true,
        leading: const Icon(Icons.person_outline, size: 20),
        title: Text(
          p.mid,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontFamily: 'monospace'),
        ),
        subtitle: Text('${p.ip}:${p.port}'),
        trailing: Text('flags ${p.flags.join(',')}'),
      ),
    );
  }
}

class _LogsTab extends ConsumerWidget {
  const _LogsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final logs = ref.watch(daemonLogsProvider);
    return Column(
      children: [
        Align(
          alignment: Alignment.centerRight,
          child: IconButton(
            tooltip: 'Rafraîchir',
            icon: const Icon(Icons.refresh, size: 18),
            onPressed: () => ref.invalidate(daemonLogsProvider),
          ),
        ),
        Expanded(
          child: logs.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              message: '$e',
              onRetry: () => ref.invalidate(daemonLogsProvider),
            ),
            data: (text) => text.trim().isEmpty
                ? const EmptyState(
                    icon: Icons.article_outlined,
                    title: 'Journal vide',
                    message: 'Le daemon n\'a encore rien écrit.',
                  )
                : SingleChildScrollView(
                    padding: const EdgeInsets.all(AppSpacing.md),
                    child: SelectableText(
                      text,
                      style: Theme.of(context).textTheme.bodySmall
                          ?.copyWith(fontFamily: 'monospace'),
                    ),
                  ),
          ),
        ),
      ],
    );
  }
}
