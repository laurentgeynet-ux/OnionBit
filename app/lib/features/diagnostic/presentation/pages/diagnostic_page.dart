// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../../downloads/presentation/providers/downloads_providers.dart';
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
    final l10n = context.l10n;
    return DefaultTabController(
      length: 11,
      child: Column(
        children: [
          TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: l10n.tabOverview),
              Tab(text: l10n.tabStats),
              Tab(text: l10n.tabOverlays),
              Tab(text: l10n.tabCircuits),
              Tab(text: l10n.tabRelays),
              Tab(text: l10n.tabExits),
              Tab(text: l10n.tabSwarms),
              Tab(text: l10n.tabPeers),
              Tab(text: l10n.tabDhtPeers),
              Tab(text: l10n.tabPexPeers),
              Tab(text: l10n.tabLogs),
            ],
          ),
          const Expanded(
            child: TabBarView(
              children: [
                _OverviewTab(),
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
              tooltip: context.l10n.refresh,
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
      emptyTitle: context.l10n.emptyOverlays,
      emptyMessage: context.l10n.emptyOverlaysMsg,
      itemBuilder: (o) => ListTile(
        dense: true,
        leading: const Icon(Icons.hub_outlined, size: 20),
        title: Text(o.name),
        subtitle: Text(o.id, overflow: TextOverflow.ellipsis),
        trailing: Text(context.l10n.peersCount(o.peers)),
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
      emptyTitle: context.l10n.emptyCircuits,
      emptyMessage: context.l10n.emptyCircuitsMsg,
      headerActions: [
        PopupMenuButton<int>(
          tooltip: context.l10n.testNewCircuit,
          icon: const Icon(Icons.speed, size: 18),
          onSelected: (hops) => SpeedTestDialog.showNewCircuit(context, hops),
          itemBuilder: (context) => [
            for (final h in const [1, 2, 3])
              PopupMenuItem(
                value: h,
                child: Text(context.l10n.testCircuitHops(h)),
              ),
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
              '${context.l10n.circuitHopCount(c.actualHops, c.goalHops)}'
              ' · ${c.state}'
              '${c.infoHash != null ? ' · ${c.infoHash}' : ''}',
            ),
            // Route réellement prise : mid hex de chaque saut, dans
            // l'ordre (+ saut en cours d'ajout suffixé « … »).
            if (c.verifiedHops.isNotEmpty)
              Text(
                context.l10n.circuitRoute(
                  [
                    for (final h in c.verifiedHops)
                      h.length > 8 ? h.substring(0, 8) : h,
                    if (c.unverifiedHop.isNotEmpty)
                      '${c.unverifiedHop.substring(0, c.unverifiedHop.length.clamp(0, 8))}…',
                  ].join(' → '),
                ),
                style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
              ),
          ],
        ),
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              '↑${context.fmtBytes(c.bytesUp)} '
              '↓${context.fmtBytes(c.bytesDown)}',
            ),
            if (_testable(c))
              IconButton(
                tooltip: context.l10n.speedTest,
                icon: const Icon(Icons.speed, size: 18),
                onPressed: () => SpeedTestDialog.showForCircuit(context, c.id),
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
      emptyTitle: context.l10n.emptyRelays,
      itemBuilder: (r) => ListTile(
        dense: true,
        title: Text('${r.circuitIn} → ${r.circuitOut}'),
        subtitle: Text(
          r.rendezvous
              ? context.l10n.relayRendezvous
              : context.l10n.relay,
        ),
        trailing: Text(
          '↑${context.fmtBytes(r.bytesUp)} '
          '↓${context.fmtBytes(r.bytesDown)}',
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
      emptyTitle: context.l10n.emptyExits,
      itemBuilder: (e) => ListTile(
        dense: true,
        leading: Icon(e.enabled ? Icons.exit_to_app : Icons.block, size: 20),
        title: Text(context.l10n.exitCircuit(e.circuitId)),
        trailing: Text(
          e.enabled
              ? context.l10n.exitActive
              : context.l10n.exitInactive,
        ),
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
      emptyTitle: context.l10n.emptySwarms,
      emptyMessage: context.l10n.emptySwarmsMsg,
      itemBuilder: (s) => ListTile(
        dense: true,
        leading: Icon(s.seeder ? Icons.upload : Icons.download, size: 20),
        title: Text(s.infoHash, overflow: TextOverflow.ellipsis),
        trailing: Text(context.l10n.connectionsCount(s.connections)),
      ),
    );
  }
}

/// `PEER_FLAG_*` IPv8/pyipv8 → nom lisible (les valeurs inconnues
/// restent affichées en numéro).
String _peerFlagLabel(AppLocalizations l10n, int flag) => switch (flag) {
  1 => l10n.flagRelay,
  2 => l10n.flagBtExit,
  4 => l10n.flagIpv8Exit,
  8 => l10n.flagSpeedTest,
  16384 => l10n.flagBackupExit,
  32768 => l10n.flagHttpExit,
  _ => '#$flag',
};

class _PeersTab extends ConsumerWidget {
  const _PeersTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return _TabScaffold<TunnelPeerInfo>(
      value: ref.watch(tunnelPeersProvider),
      onRetry: () => ref.invalidate(tunnelPeersProvider),
      emptyTitle: context.l10n.emptyTunnelPeers,
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
              ? context.l10n.noFlags
              : p.flags
                    .map((f) => _peerFlagLabel(context.l10n, f))
                    .join(' · '),
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
      emptyMessage: context.l10n.emptyIntroPointsMsg,
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
          subtitle: Text(context.l10n.introPointsCount(s.peers.length)),
          children: [
            for (final p in s.peers)
              ListTile(
                dense: true,
                leading: const Icon(Icons.person_pin_outlined, size: 18),
                title: Text('${p.ip}:${p.port}'),
                subtitle: Text(
                  context.l10n.seederPrefix(
                    p.seederPk.length > 12
                        ? '${p.seederPk.substring(0, 12)}…'
                        : p.seederPk,
                  ),
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
    emptyTitle: context.l10n.emptyDhtIntro,
  );
}

class _PexPeersTab extends StatelessWidget {
  const _PexPeersTab();

  @override
  Widget build(BuildContext context) => _SwarmPeersTab(
    provider: pexPeersProvider,
    emptyTitle: context.l10n.emptyPexIntro,
  );
}

/// Onglet « Statistiques » — compteurs globaux du daemon
/// (`/api/statistics/tribler` : taille DB, torrents, canaux, pairs,
/// sessions moteur).
class _StatsTab extends ConsumerWidget {
  const _StatsTab();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final stats = ref.watch(onionbitStatsProvider);
    return Column(
      children: [
        Align(
          alignment: Alignment.centerRight,
          child: IconButton(
            tooltip: context.l10n.refresh,
            icon: const Icon(Icons.refresh, size: 18),
            onPressed: () => ref.invalidate(onionbitStatsProvider),
          ),
        ),
        Expanded(
          child: stats.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              message: '$e',
              onRetry: () => ref.invalidate(onionbitStatsProvider),
            ),
            data: (s) => ListView(
              padding: const EdgeInsets.all(AppSpacing.md),
              children: [
                _stat(context, context.l10n.statDaemonVersion, s.version),
                _stat(
                  context,
                  context.l10n.statDbSize,
                  context.fmtBytes(s.dbSize),
                ),
                _stat(
                  context,
                  context.l10n.statTorrentsKnown,
                  '${s.numTorrents}',
                ),

                _stat(
                  context,
                  context.l10n.statIpv8Peers,
                  s.peers < 0 ? '—' : '${s.peers}',
                ),
                _stat(
                  context,
                  context.l10n.statSessions,
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
              message: context.l10n.logsDebugTooltip,
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
              tooltip: context.l10n.refresh,
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: () => ref.invalidate(daemonLogsProvider),
            ),
          ],
        ),
        if (uiLog.isNotEmpty)
          ExpansionTile(
            dense: true,
            title: Text(
              context.l10n.uiLogTitle,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            subtitle: Text(
              context.l10n.uiLogSubtitle,
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
                ? EmptyState(
                    icon: Icons.article_outlined,
                    title: context.l10n.emptyLogs,
                    message: context.l10n.emptyLogsMsg,
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

/// Onglet « Vue d'ensemble » — tableau de bord des compteurs clés du
/// daemon (overlays, circuits, relais, sorties, pairs tunnel,
/// torrents, débits globaux) + pastille de santé. Auto-refresh 5 s —
/// la page diagnostic est faite pour être surveillée.
class _OverviewTab extends ConsumerStatefulWidget {
  const _OverviewTab();

  @override
  ConsumerState<_OverviewTab> createState() => _OverviewTabState();
}

class _OverviewTabState extends ConsumerState<_OverviewTab> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 5), (_) {
      if (!mounted) return;
      ref.invalidate(overlaysProvider);
      ref.invalidate(tunnelCircuitsProvider);
      ref.invalidate(tunnelRelaysProvider);
      ref.invalidate(tunnelExitsProvider);
      ref.invalidate(tunnelPeersProvider);
      ref.invalidate(onionbitStatsProvider);
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  void _refresh() {
    ref.invalidate(overlaysProvider);
    ref.invalidate(tunnelCircuitsProvider);
    ref.invalidate(tunnelRelaysProvider);
    ref.invalidate(tunnelExitsProvider);
    ref.invalidate(tunnelPeersProvider);
    ref.invalidate(onionbitStatsProvider);
  }

  @override
  Widget build(BuildContext context) {
    final overlays = ref.watch(overlaysProvider).value;
    final circuits = ref.watch(tunnelCircuitsProvider).value;
    final relays = ref.watch(tunnelRelaysProvider).value;
    final exits = ref.watch(tunnelExitsProvider).value;
    final peers = ref.watch(tunnelPeersProvider).value;
    final stats = ref.watch(onionbitStatsProvider).value;
    final speeds = ref.watch(totalSpeedsProvider);

    final ready = circuits?.where((c) => c.ready).length ?? 0;
    final exitsOn = exits?.where((e) => e.enabled).length ?? 0;
    final tunnelsUp = (overlays ?? const []).isNotEmpty;
    final healthy = tunnelsUp && ready > 0;
    final partial = tunnelsUp;

    final scheme = Theme.of(context).colorScheme;
    final healthColor = healthy
        ? Colors.green
        : partial
        ? scheme.tertiary
        : scheme.error;

    return Column(
      children: [
        Row(
          children: [
            const SizedBox(width: AppSpacing.md),
            Icon(Icons.circle, size: 10, color: healthColor),
            const SizedBox(width: AppSpacing.xs),
            Text(
              !tunnelsUp
                  ? context.l10n.overviewTunnelInactive
                  : ready > 0
                  ? context.l10n.overviewTunnelsOk(ready)
                  : context.l10n.overviewTunnelNoCircuit,
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const Spacer(),
            Text(
              context.l10n.autoRefresh5s,
              style: Theme.of(context).textTheme.labelSmall
                  ?.copyWith(color: scheme.outline),
            ),
            IconButton(
              tooltip: context.l10n.refresh,
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: _refresh,
            ),
          ],
        ),
        Expanded(
          child: GridView.count(
            crossAxisCount: 3,
            childAspectRatio: 3.2,
            padding: const EdgeInsets.all(AppSpacing.md),
            mainAxisSpacing: AppSpacing.sm,
            crossAxisSpacing: AppSpacing.sm,
            children: [
              _StatCard(
                icon: Icons.hub_outlined,
                label: context.l10n.cardIpv8Overlays,
                value: '${overlays?.length ?? '—'}',
              ),
              _StatCard(
                icon: Icons.link,
                label: context.l10n.cardCircuitsReady,
                value: '$ready / ${circuits?.length ?? '—'}',
              ),
              _StatCard(
                icon: Icons.swap_horiz,
                label: context.l10n.cardRelays,
                value: '${relays?.length ?? '—'}',
              ),
              _StatCard(
                icon: Icons.exit_to_app,
                label: context.l10n.cardActiveExits,
                value: '$exitsOn / ${exits?.length ?? '—'}',
              ),
              _StatCard(
                icon: Icons.person_outline,
                label: context.l10n.cardTunnelPeers,
                value: '${peers?.length ?? '—'}',
              ),
              _StatCard(
                icon: Icons.cloud_download_outlined,
                label: context.l10n.cardKnownTorrents,
                value: '${stats?.numTorrents ?? '—'}',
              ),
              _StatCard(
                icon: Icons.arrow_downward,
                label: context.l10n.cardDownload,
                value: context.fmtRate(speeds.down),
              ),
              _StatCard(
                icon: Icons.arrow_upward,
                label: context.l10n.cardUpload,
                value: context.fmtRate(speeds.up),
              ),
              _StatCard(
                icon: Icons.storage_outlined,
                label: context.l10n.cardDatabase,
                value: stats != null ? context.fmtBytes(stats.dbSize) : '—',
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Carte compteur du tableau de bord « Vue d'ensemble ».
class _StatCard extends StatelessWidget {
  const _StatCard({
    required this.icon,
    required this.label,
    required this.value,
  });

  final IconData icon;
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.md),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Row(
              children: [
                Icon(icon, size: 16, color: theme.colorScheme.outline),
                const SizedBox(width: AppSpacing.xs),
                Expanded(
                  child: Text(
                    label,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(value, style: theme.textTheme.titleMedium),
          ],
        ),
      ),
    );
  }
}
