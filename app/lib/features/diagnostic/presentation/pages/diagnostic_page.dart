// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../../downloads/presentation/providers/downloads_providers.dart';
import '../../../settings/presentation/providers/settings_providers.dart';
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
      length: 12,
      child: Column(
        children: [
          TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [
              Tab(text: l10n.tabOverview),
              Tab(text: l10n.tabStats),
              Tab(text: l10n.tabConnections),
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
                _ConnectionsTab(),
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
            error: (e, _) => ErrorState(error: e, onRetry: onRetry),
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

/// Onglet « Connexions » — vue agrégée `GET /api/connections`
/// (extension Rust) : chaque adresse distante `ip:port` avec les
/// rôles observés (IPv8/UDP, DHT, tunnel, sorties, BitTorrent
/// TCP/uTP/SOCKS) + les sockets d'écoute locales.
class _ConnectionsTab extends ConsumerWidget {
  const _ConnectionsTab();

  /// Pastille de rôle (transport/protocole) dans le sous-titre.
  Widget _chip(BuildContext context, String label, {IconData? icon}) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(4),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 11, color: scheme.outline),
            const SizedBox(width: 2),
          ],
          Text(label, style: Theme.of(context).textTheme.labelSmall),
        ],
      ),
    );
  }

  String _listenerLabel(AppLocalizations l10n, ListenerInfo l) =>
      switch (l.protocol) {
        'ipv8-udp' => 'IPv8 UDP',
        'ipv8-udp-v6' => 'IPv8 UDPv6',
        'bittorrent' => 'BitTorrent TCP/uTP',
        'tunnel-exit-udp' => l10n.listenerExitUdp(l.circuitId ?? 0),
        'socks5' => l10n.listenerSocks5(l.hops ?? 0),
        _ => l.protocol,
      };

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final report = ref.watch(connectionsProvider);
    final l10n = context.l10n;
    return Column(
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            IconButton(
              tooltip: l10n.refresh,
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: () => ref.invalidate(connectionsProvider),
            ),
          ],
        ),
        Expanded(
          child: report.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              error: e,
              onRetry: () => ref.invalidate(connectionsProvider),
            ),
            data: (r) => r.connections.isEmpty && r.listeners.isEmpty
                ? EmptyState(
                    icon: Icons.lan_outlined,
                    title: l10n.emptyConnections,
                    message: l10n.emptyConnectionsMsg,
                  )
                : ListView(
                    padding: const EdgeInsets.symmetric(
                      horizontal: AppSpacing.md,
                    ),
                    children: [
                      if (r.listeners.isNotEmpty) ...[
                        Padding(
                          padding: const EdgeInsets.only(bottom: AppSpacing.xs),
                          child: Text(
                            l10n.listenersSection,
                            style: Theme.of(context).textTheme.labelLarge
                                ?.copyWith(
                                  color: Theme.of(context).colorScheme.primary,
                                ),
                          ),
                        ),
                        for (final l in r.listeners)
                          ListTile(
                            dense: true,
                            leading: const Icon(Icons.hearing, size: 18),
                            title: Text(
                              l.address,
                              style: const TextStyle(
                                fontFamily: 'monospace',
                                fontSize: 12,
                              ),
                            ),
                            trailing: Text(_listenerLabel(l10n, l)),
                          ),
                        const Divider(),
                      ],
                      for (final c in r.connections) _connTile(context, c),
                    ],
                  ),
          ),
        ),
      ],
    );
  }

  Widget _connTile(BuildContext context, ConnectionInfo c) {
    final l10n = context.l10n;
    final chips = <Widget>[
      for (final t in c.transports)
        _chip(context, t.toUpperCase(), icon: Icons.swap_vert),
      if (c.ipv8) _chip(context, 'IPv8'),
      if (c.dht) _chip(context, 'DHT'),
      for (final f in c.tunnelFlags) _chip(context, _peerFlagLabel(l10n, f)),
      for (final cid in c.exitCircuits)
        _chip(context, l10n.connExitTarget(cid)),
    ];
    return ExpansionTile(
      dense: true,
      leading: Icon(
        c.bittorrent.isNotEmpty
            ? Icons.download_done
            : c.ipv8
            ? Icons.hub_outlined
            : Icons.public,
        size: 20,
      ),
      title: Text(
        '${c.ip}:${c.port}',
        style: const TextStyle(fontFamily: 'monospace', fontSize: 13),
      ),
      subtitle: chips.isEmpty
          ? null
          : Wrap(spacing: AppSpacing.xs, runSpacing: 2, children: chips),
      children: [
        for (final b in c.bittorrent)
          ListTile(
            dense: true,
            leading: Icon(
              b.incoming ? Icons.south_west : Icons.north_east,
              size: 16,
            ),
            title: Text(
              'BitTorrent ${b.connKind.toUpperCase()} · ${b.state}'
              '${b.client.isNotEmpty ? ' · ${b.client}' : ''}',
            ),
            subtitle: Text(
              b.infohash.length > 16
                  ? '${b.infohash.substring(0, 16)}…'
                  : b.infohash,
              style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
            ),
            trailing: Text(
              '↑${context.fmtBytes(b.bytesUp)} '
              '↓${context.fmtBytes(b.bytesDown)}',
            ),
          ),
        if (c.mid.isNotEmpty)
          ListTile(
            dense: true,
            leading: const Icon(Icons.key_outlined, size: 16),
            title: Text(
              l10n.connMid(
                c.mid.length > 16 ? '${c.mid.substring(0, 16)}…' : c.mid,
              ),
              style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
            ),
          ),
        if (c.overlays.isNotEmpty)
          ListTile(
            dense: true,
            leading: const Icon(Icons.hub_outlined, size: 16),
            title: Text(c.overlays.join(' · ')),
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
          r.rendezvous ? context.l10n.relayRendezvous : context.l10n.relay,
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
          e.enabled ? context.l10n.exitActive : context.l10n.exitInactive,
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
              : p.flags.map((f) => _peerFlagLabel(context.l10n, f)).join(' · '),
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

/// Onglet « Statistiques » — compteurs globaux du daemon regroupés
/// par domaine : `GET /api/statistics/tribler` (version, uptime, DB,
/// totaux session, lanes), `GET /api/statistics/ipv8` (trafic
/// overlay), `PUT /api/statistics/dirspace` (espace du dossier de
/// téléchargement) et dérivés locaux (downloads, circuits DATA,
/// sorties actives).
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
            onPressed: () => ref
              ..invalidate(onionbitStatsProvider)
              ..invalidate(ipv8TrafficProvider),
          ),
        ),
        Expanded(
          child: stats.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              error: e,
              onRetry: () => ref.invalidate(onionbitStatsProvider),
            ),
            data: (s) => _statsList(context, ref, s),
          ),
        ),
      ],
    );
  }

  Widget _statsList(BuildContext context, WidgetRef ref, OnionbitStats s) {
    final l10n = context.l10n;
    final traffic = ref.watch(ipv8TrafficProvider).value;
    final circuits = ref.watch(tunnelCircuitsProvider).value ?? const [];
    final exits = ref.watch(tunnelExitsProvider).value ?? const [];
    final downloads = ref.watch(downloadsProvider).value ?? const [];

    // Dossier de destination pour l'espace disque (premier ancêtre
    // existant mesuré par `PUT /api/statistics/dirspace`).
    final settings = ref.watch(daemonSettingsProvider).value;
    final lt = settings?['libtorrent'] as Map<String, dynamic>?;
    final saveas =
        (lt?['download_defaults'] as Map<String, dynamic>?)?['saveas']
            as String?;
    final disk = ref.watch(
      dirSpaceProvider(saveas == null || saveas.isEmpty ? null : saveas),
    );

    // Circuits DATA prêts, groupés par lane (« ×2 : 3 »).
    final dataReady = <int, int>{};
    for (final c in circuits.where((c) => c.type == 'DATA' && c.ready)) {
      dataReady[c.goalHops] = (dataReady[c.goalHops] ?? 0) + 1;
    }
    final dataReadyText = dataReady.isEmpty
        ? l10n.statNone
        : (dataReady.entries.toList()..sort((a, b) => a.key.compareTo(b.key)))
              .map((e) => '×${e.key} : ${e.value}')
              .join(' · ');

    final activeExits = exits.where((e) => e.enabled).length;

    return ListView(
      padding: const EdgeInsets.all(AppSpacing.md),
      children: [
        _section(context, l10n.sectionDaemon),
        _stat(context, l10n.statDaemonVersion, s.version),
        _stat(
          context,
          l10n.statUptime,
          s.uptimeSec < 0
              ? '—'
              : context.fmtDuration(Duration(seconds: s.uptimeSec)),
        ),
        _stat(context, l10n.statDbSize, context.fmtBytes(s.dbSize)),
        _stat(
          context,
          l10n.statDiskSpace,
          disk.when(
            loading: () => '…',
            error: (_, _) => '—',
            data: (d) => l10n.diskSpaceFree(
              context.fmtBytes(d['free'] ?? 0),
              context.fmtBytes(d['total'] ?? 0),
            ),
          ),
        ),
        _section(context, l10n.statSectionContent),
        _stat(context, l10n.statTorrentsKnown, '${s.numTorrents}'),
        _stat(
          context,
          l10n.statDownloads,
          l10n.statDownloadsValue(
            downloads.where((d) => d.isActive).length,
            downloads.where((d) => d.isPaused).length,
            downloads.where((d) => d.isError).length,
          ),
        ),
        _section(context, l10n.statSectionNetwork),
        _stat(context, l10n.statIpv8Peers, s.peers < 0 ? '—' : '${s.peers}'),
        _stat(
          context,
          l10n.statIpv8Traffic,
          traffic == null
              ? '—'
              : l10n.statTrafficValue(
                  context.fmtBytes(traffic.up),
                  context.fmtBytes(traffic.down),
                ),
        ),
        _stat(
          context,
          l10n.statSessionTraffic,
          s.totalSentBytes < 0
              ? '—'
              : l10n.statTrafficValue(
                  context.fmtBytes(s.totalSentBytes),
                  context.fmtBytes(s.totalRecvBytes),
                ),
        ),
        _section(context, l10n.rowAnon),
        _stat(
          context,
          l10n.statSessions,
          s.sessions < 0 ? '—' : '${s.sessions}',
        ),
        _stat(
          context,
          l10n.statLanes,
          s.laneHops.isEmpty
              ? l10n.statNone
              : ([...s.laneHops]..sort()).map((h) => '×$h').join(' · '),
        ),
        _stat(context, l10n.statDataCircuits, dataReadyText),
        _stat(context, l10n.statExitsActive, '$activeExits'),
        // Débit servi aux autres pairs (contrôleur de congestion —
        // `tunnel_community/bandwidth`, extension Rust).
        if (traffic?.bandwidth case final bw?) ...[
          _stat(
            context,
            l10n.statBwRtt,
            bw.minRttMs != null
                ? l10n.statBwRttValue(
                    bw.minRttMs!.toStringAsFixed(0),
                    bw.baseRttMs?.toStringAsFixed(0) ?? '—',
                    bw.rttSamples,
                  )
                : l10n.bwSourcePending,
          ),
          _stat(
            context,
            l10n.statBwServed,
            bw.effectiveRelayBps > 0
                ? context.fmtRate(bw.effectiveRelayBps)
                : bw.relayMode == 'unlimited'
                ? l10n.bwUnlimited
                : '—',
          ),
          _stat(
            context,
            l10n.statBwMeasuredRate,
            '${context.fmtRate(bw.servedBps)} '
            '(${context.fmtBytes(bw.servedBytes)})',
          ),
          if (bw.relayDropped > 0)
            _stat(context, l10n.statBwDropped, '${bw.relayDropped}'),
        ],
      ],
    );
  }

  Widget _section(BuildContext context, String title) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: AppSpacing.md, bottom: AppSpacing.xs),
      child: Text(
        title,
        style: theme.textTheme.labelLarge?.copyWith(
          color: theme.colorScheme.primary,
        ),
      ),
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
              error: e,
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
/// torrents, débits globaux) + pastille de santé. Le refresh vient
/// du `tickProvider` interne des providers (5 s) — pas de timer
/// local (doublon historique qui ajoutait une rafale d'invalidations
/// par-dessus le sondage de chaque provider).
class _OverviewTab extends ConsumerWidget {
  const _OverviewTab();

  void _refresh(WidgetRef ref) {
    ref.invalidate(overlaysProvider);
    ref.invalidate(tunnelCircuitsProvider);
    ref.invalidate(tunnelRelaysProvider);
    ref.invalidate(tunnelExitsProvider);
    ref.invalidate(tunnelPeersProvider);
    ref.invalidate(onionbitStatsProvider);
    ref.invalidate(ipv8TrafficProvider);
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final overlays = ref.watch(overlaysProvider).value;
    final circuits = ref.watch(tunnelCircuitsProvider).value;
    final relays = ref.watch(tunnelRelaysProvider).value;
    final exits = ref.watch(tunnelExitsProvider).value;
    final peers = ref.watch(tunnelPeersProvider).value;
    final stats = ref.watch(onionbitStatsProvider).value;
    final speeds = ref.watch(totalSpeedsProvider);
    final tunnel = ref.watch(ipv8TrafficProvider).value;

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
              onPressed: () => _refresh(ref),
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
                label: context.l10n.cardFilesDownload,
                value: context.fmtRate(speeds.down),
              ),
              _StatCard(
                icon: Icons.arrow_upward,
                label: context.l10n.cardFilesUpload,
                value: context.fmtRate(speeds.up),
              ),
              _StatCard(
                icon: Icons.swap_vert,
                label: context.l10n.cardTunnelTraffic,
                value: tunnel == null
                    ? '—'
                    : '↓ ${context.fmtRate(tunnel.rateDown)} · '
                          '↑ ${context.fmtRate(tunnel.rateUp)}',
                caption: tunnel == null
                    ? null
                    : [
                        // Debit servi aux autres pairs, mesure exacte
                        // au limiteur (extension Rust) — repond a
                        // « combien je relaye » sans soustraire le
                        // trafic propre.
                        if (tunnel.bandwidth case final bw?)
                          context.l10n.cardRelayServed(
                            context.fmtRate(bw.servedBps),
                          ),
                        context.l10n.statTrafficValue(
                          context.fmtBytes(tunnel.up),
                          context.fmtBytes(tunnel.down),
                        ),
                      ].join('\n'),
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

/// Carte compteur du tableau de bord « Vue d'ensemble » — `caption`
/// optionnelle en seconde ligne (cumuls, détails).
class _StatCard extends StatelessWidget {
  const _StatCard({
    required this.icon,
    required this.label,
    required this.value,
    this.caption,
  });

  final IconData icon;
  final String label;
  final String value;
  final String? caption;

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
            if (caption case final c?)
              Text(
                c,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
                overflow: TextOverflow.ellipsis,
              ),
          ],
        ),
      ),
    );
  }
}
