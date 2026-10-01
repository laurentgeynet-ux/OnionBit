// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/diagnostic/domain/diagnostic_models.dart';
import '../../features/diagnostic/presentation/providers/diagnostic_providers.dart';
import '../../features/downloads/presentation/providers/downloads_providers.dart';
import '../di/providers.dart';
import '../theme/app_theme.dart';
import '../utils/byte_formatter.dart';

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
    final lane = ref.watch(anonLaneProvider).value;
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
            connected ? 'Daemon connecté' : 'Daemon injoignable',
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
                      AnonLaneState.ready =>
                        'Anonyme : ${lane!.readyCircuits} circuit(s) prêt(s)',
                      AnonLaneState.waiting =>
                        'Anonyme : en attente de circuit',
                      AnonLaneState.disabled => 'Anonyme : désactivé',
                      null => 'Anonyme : …',
                    },
                    style: small,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: AppSpacing.md),
          Text('↓ ${ByteFormatter.formatRate(speeds.down)}', style: small),
          const SizedBox(width: AppSpacing.md),
          Text('↑ ${ByteFormatter.formatRate(speeds.up)}', style: small),
        ],
      ),
    );
  }
}
