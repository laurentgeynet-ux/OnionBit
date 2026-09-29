import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/diagnostic_models.dart';
import '../providers/diagnostic_providers.dart';
import '../widgets/speed_test_dialog.dart';

/// Page « Diagnostic » — tout ce qui est interne au réseau vit ici
/// (overlays, circuits, relais, sorties, swarms, pairs, journaux),
/// jamais dans le parcours principal.
class DiagnosticPage extends StatelessWidget {
  const DiagnosticPage({super.key});

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 10,
      child: Column(
        children: [
          const TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: 'Statistiques'),
              Tab(text: 'Overlays'),
              Tab(text: 'Circuits'),
              Tab(text: 'Relais'),
              Tab(text: 'Sorties'),
              Tab(text: 'Swarms'),
              Tab(text: 'Pairs'),
              Tab(text: 'Pairs DHT'),
              Tab(text: 'Pairs PEX'),
              Tab(text: 'Journaux'),
            ],
          ),
          const Expanded(
            child: TabBarView(
              children: [
                _StatsTab(),
                _OverlaysTab(),
                _CircuitsTab(),
                _RelaysTab(),
                _ExitsTab(),
                _SwarmsTab(),
                _PeersTab(),
                _DhtPeersTab(),
                _PexPeersTab(),
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
    this.headerActions = const [],
  });

  final AsyncValue<List<T>> value;
  final VoidCallback onRetry;
  final Widget Function(T item) itemBuilder;
  final String emptyTitle;
  final String? emptyMessage;

  /// Actions supplémentaires dans la ligne d'en-tête (à gauche du
  /// bouton « Rafraîchir »).
  final List<Widget> headerActions;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            ...headerActions,
            IconButton(
              tooltip: 'Rafraîchir',
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: onRetry,
            ),
          ],
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

  /// `PEER_FLAG_SPEED_TEST` (`exit_policy.rs` = 8) : requis sur un
  /// circuit `DATA` pour lancer un speed test ; les autres types de
  /// circuits sont toujours testables (`speed_test_existing_circuit`).
  static const int _peerFlagSpeedTest = 8;

  bool _testable(CircuitInfo c) =>
      c.ready && (c.type != 'DATA' || c.exitFlags & _peerFlagSpeedTest != 0);

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<CircuitInfo>(
      value: ref.watch(tunnelCircuitsProvider),
      onRetry: () => ref.invalidate(tunnelCircuitsProvider),
      emptyTitle: 'Aucun circuit',
      emptyMessage:
          'Les circuits anonymes sont construits quand un '
          'téléchargement en demande.',
      headerActions: [
        PopupMenuButton<int>(
          tooltip: 'Tester un nouveau circuit',
          icon: const Icon(Icons.speed, size: 18),
          onSelected: (hops) => SpeedTestDialog.showNewCircuit(context, hops),
          itemBuilder: (_) => const [
            PopupMenuItem(value: 1, child: Text('Tester un circuit à 1 saut')),
            PopupMenuItem(value: 2, child: Text('Tester un circuit à 2 sauts')),
            PopupMenuItem(value: 3, child: Text('Tester un circuit à 3 sauts')),
          ],
        ),
      ],
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
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              '↑${ByteFormatter.format(c.bytesUp)} '
              '↓${ByteFormatter.format(c.bytesDown)}',
            ),
            if (_testable(c))
              IconButton(
                tooltip: 'Test de vitesse',
                icon: const Icon(Icons.speed, size: 18),
                onPressed: () =>
                    SpeedTestDialog.showForCircuit(context, c.id),
              ),
          ],
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

/// `PEER_FLAG_*` IPv8/pyipv8 → nom lisible (les valeurs inconnues
/// restent affichées en numéro).
String _peerFlagLabel(int flag) => switch (flag) {
  1 => 'relais',
  2 => 'sortie-bt',
  4 => 'sortie-ipv8',
  8 => 'speed-test',
  16384 => 'sortie-backup',
  32768 => 'sortie-http',
  _ => '#$flag',
};

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
        trailing: Text(
          p.flags.isEmpty
              ? 'aucun flag'
              : p.flags.map(_peerFlagLabel).join(' · '),
        ),
      ),
    );
  }
}

/// Liste des points d'introduction groupés par swarm (vues « Pairs
/// DHT » et « Pairs PEX » — même shape `[{info_hash, peers}]`).
class _SwarmPeersTab extends ConsumerWidget {
  const _SwarmPeersTab({required this.provider, required this.emptyTitle});

  final FutureProvider<List<SwarmPeers>> provider;
  final String emptyTitle;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<SwarmPeers>(
      value: ref.watch(provider),
      onRetry: () => ref.invalidate(provider),
      emptyTitle: emptyTitle,
      emptyMessage:
          'Apparaît quand des points d\'introduction de swarms '
          'cachés sont connus.',
      itemBuilder: (s) => Card(
        margin: const EdgeInsets.only(bottom: AppSpacing.sm),
        child: ExpansionTile(
          dense: true,
          leading: const Icon(Icons.hub_outlined, size: 20),
          title: Text(
            s.infoHash,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
          ),
          subtitle: Text('${s.peers.length} point(s) d\'introduction'),
          children: [
            for (final p in s.peers)
              ListTile(
                dense: true,
                leading: const Icon(Icons.person_pin_outlined, size: 18),
                title: Text('${p.ip}:${p.port}'),
                subtitle: Text(
                  'seeder ${p.seederPk.length > 12 ? '${p.seederPk.substring(0, 12)}…' : p.seederPk}',
                  style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
                ),
                trailing: Text(p.source),
              ),
          ],
        ),
      ),
    );
  }
}

class _DhtPeersTab extends StatelessWidget {
  const _DhtPeersTab();

  @override
  Widget build(BuildContext context) => _SwarmPeersTab(
    provider: dhtPeersProvider,
    emptyTitle: 'Aucun point d\'introduction DHT',
  );
}

class _PexPeersTab extends StatelessWidget {
  const _PexPeersTab();

  @override
  Widget build(BuildContext context) => _SwarmPeersTab(
    provider: pexPeersProvider,
    emptyTitle: 'Aucun point d\'introduction PEX',
  );
}

/// Onglet « Statistiques » — compteurs globaux du daemon
/// (`/api/statistics/tribler` : taille DB, torrents, canaux, pairs,
/// sessions moteur).
class _StatsTab extends ConsumerWidget {
  const _StatsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final stats = ref.watch(triblerStatsProvider);
    return Column(
      children: [
        Align(
          alignment: Alignment.centerRight,
          child: IconButton(
            tooltip: 'Rafraîchir',
            icon: const Icon(Icons.refresh, size: 18),
            onPressed: () => ref.invalidate(triblerStatsProvider),
          ),
        ),
        Expanded(
          child: stats.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              message: '$e',
              onRetry: () => ref.invalidate(triblerStatsProvider),
            ),
            data: (s) => ListView(
              padding: const EdgeInsets.all(AppSpacing.md),
              children: [
                _stat(context, 'Version du daemon', s.version),
                _stat(
                  context,
                  'Taille de la base',
                  ByteFormatter.format(s.dbSize),
                ),
                _stat(context, 'Torrents connus', '${s.numTorrents}'),
                _stat(context, 'Canaux', '${s.numChannels}'),
                _stat(
                  context,
                  'Pairs IPv8 découverts',
                  s.peers < 0 ? '—' : '${s.peers}',
                ),
                _stat(
                  context,
                  'Sessions moteur (direct + lanes)',
                  s.sessions < 0 ? '—' : '${s.sessions}',
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Widget _stat(BuildContext context, String label, String value) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: AppSpacing.xs),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          Text(value, style: theme.textTheme.titleMedium),
        ],
      ),
    );
  }
}

class _LogsTab extends ConsumerWidget {
  const _LogsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final logs = ref.watch(daemonLogsProvider);
    final uiLog = ref.watch(uiConnectLogProvider).value ?? '';
    final debug = ref.watch(debugLogEnabledProvider);
    return Column(
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            // `PUT /api/ipv8/asyncio/debug` : bascule le filtre de log
            // à `debug` à chaud — rend visibles les événements de
            // cellules tunnel (create/extend/destroy, e2e, sorties).
            Tooltip(
              message:
                  'Journalise les événements tunnel détaillés '
                  '(create/extend/destroy, e2e…)',
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text('Debug', style: Theme.of(context).textTheme.bodySmall),
                  Switch(
                    value: debug.value ?? false,
                    onChanged: debug.isLoading
                        ? null
                        : (v) async {
                            await ref
                                .read(diagnosticRepositoryProvider)
                                .setDebug(v);
                            ref.invalidate(debugLogEnabledProvider);
                          },
                  ),
                ],
              ),
            ),
            IconButton(
              tooltip: 'Rafraîchir',
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: () => ref.invalidate(daemonLogsProvider),
            ),
          ],
        ),
        if (uiLog.isNotEmpty)
          ExpansionTile(
            dense: true,
            title: Text(
              'Journal UI',
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            subtitle: Text(
              'logs/ui.log — connexion daemon, SSE, ajouts, recherches',
              style: Theme.of(context).textTheme.bodySmall,
            ),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: Padding(
                  padding: const EdgeInsets.all(AppSpacing.md),
                  child: SelectableText(
                    uiLog,
                    style: Theme.of(context).textTheme.bodySmall
                        ?.copyWith(fontFamily: 'monospace'),
                  ),
                ),
              ),
            ],
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
