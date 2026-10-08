// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/diagnostic/domain/diagnostic_models.dart';
import '../../features/diagnostic/presentation/providers/diagnostic_providers.dart';
import '../../features/downloads/presentation/providers/downloads_providers.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../theme/app_theme.dart';

/// Barre d'état inférieure : connexion daemon (SSE), état honnête de
/// la lane anonyme (circuits `READY`, jamais la seule joignabilité du
/// proxy), débits globaux.
class StatusBar extends ConsumerWidget {
  const StatusBar({super.key});

  static const double height = 30;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final connected = ref.watch(sseConnectedProvider).value ?? false;
    // Watchdog : re-resolution de l'URL du daemon tant que le SSE est
    // coupe (port `http_port_running` périmé au redémarrage).
    ref.watch(connectionWatchdogProvider);
    final speeds = ref.watch(totalSpeedsProvider);
    final sessionTraffic = ref.watch(sessionTrafficProvider);
    final lane = ref.watch(anonLaneProvider).value;
    // Plafond du debit servi aux autres pairs (estimateur de
    // capacite upload — `tunnel_community/bandwidth`).
    final relayBw =
        ref.watch(ipv8TrafficProvider).value?.bandwidth;
    final small = theme.textTheme.bodySmall;

    return Container(
      height: StatusBar.height,
      padding: const EdgeInsets.symmetric(horizontal: AppSpacing.md),
      decoration: BoxDecoration(
        color: scheme.surface,
        border: Border(top: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Row(
        children: [
          Icon(
            Icons.circle,
            size: 8,
            color: connected ? Colors.green : scheme.error,
          ),
          const SizedBox(width: AppSpacing.xs),
          Text(
            connected
                ? context.l10n.daemonConnected
                : context.l10n.daemonUnreachable,
            style: small,
          ),
          const SizedBox(width: AppSpacing.lg),
          Expanded(
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.shield_outlined,
                  size: 14,
                  color: switch (lane?.state) {
                    AnonLaneState.ready => Colors.green,
                    AnonLaneState.waiting => scheme.tertiary,
                    _ => scheme.outline,
                  },
                ),
                const SizedBox(width: AppSpacing.xs),
                Flexible(
                  child: Text(
                    switch (lane?.state) {
                      AnonLaneState.ready => context.l10n.statusAnonReady(
                        lane!.readyCircuits,
                      ),
                      AnonLaneState.waiting => context.l10n.statusAnonWaiting,
                      AnonLaneState.disabled => context.l10n.statusAnonDisabled,
                      null => context.l10n.statusAnonLoading,
                    },
                    style: small,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: AppSpacing.md),
          Text('↓ ${context.fmtRate(speeds.down)}', style: small),
          const SizedBox(width: AppSpacing.md),
          Text('↑ ${context.fmtRate(speeds.up)}', style: small),
          const SizedBox(width: AppSpacing.md),
          Flexible(
            child: Tooltip(
              message: context.l10n.statusSessionTrafficTip,
              child: Text(
                context.l10n.statusSessionTraffic(
                  context.fmtBytes(sessionTraffic.down),
                  context.fmtBytes(sessionTraffic.up),
                ),
                style: small?.copyWith(color: scheme.outline),
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ),
          if (relayBw != null && relayBw.effectiveRelayBps > 0) ...[
            const SizedBox(width: AppSpacing.md),
            Tooltip(
              message: context.l10n.statusRelayCapTip,
              child: Text(
                relayBw.minRttMs != null
                    ? context.l10n.statusRelayCapRtt(
                        context.fmtRate(relayBw.effectiveRelayBps),
                        relayBw.minRttMs!.toStringAsFixed(0),
                      )
                    : context.l10n.statusRelayCap(
                        context.fmtRate(relayBw.effectiveRelayBps),
                      ),
                style: small?.copyWith(color: scheme.outline),
              ),
            ),
          ],
        ],
      ),
    );
  }
}
