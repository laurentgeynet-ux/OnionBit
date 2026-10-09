// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import '../../domain/diagnostic_models.dart';
import '../providers/diagnostic_providers.dart';

/// Dialogue « Test de vitesse de circuit » — consomme le flux
/// `speed: {up, down}` pyipv8 (MiB/s) et affiche le dernier
/// échantillon en direct. `circuitId` nul = nouveau circuit
/// `SPEED_TEST` temporaire de `hops` sauts.
class SpeedTestDialog extends ConsumerStatefulWidget {
  const SpeedTestDialog({super.key, this.circuitId, this.hops = 1});

  /// Circuit existant à tester (`null` = circuit temporaire).
  final int? circuitId;

  /// Sauts du circuit temporaire (ignoré si `circuitId` non nul).
  final int hops;

  /// Test sur un circuit existant (`/circuits/{id}/test`).
  static Future<void> showForCircuit(BuildContext context, int circuitId) =>
      showDialog(
        context: context,
        builder: (_) => SpeedTestDialog(circuitId: circuitId),
      );

  /// Test sur un circuit temporaire (`/circuits/test?goal_hops=h`).
  static Future<void> showNewCircuit(BuildContext context, int hops) =>
      showDialog(
        context: context,
        builder: (_) => SpeedTestDialog(hops: hops),
      );

  @override
  ConsumerState<SpeedTestDialog> createState() => _SpeedTestDialogState();
}

class _SpeedTestDialogState extends ConsumerState<SpeedTestDialog> {
  StreamSubscription<SpeedSample>? _sub;
  SpeedSample? _last;
  String? _error;
  bool _done = false;

  @override
  void initState() {
    super.initState();
    _sub = _stream().listen(
      (s) => setState(() => _last = s),
      onError: (Object e) => setState(() {
        _error = '$e';
        _done = true;
      }),
      onDone: () => setState(() => _done = true),
    );
  }

  Stream<SpeedSample> _stream() {
    final repo = ref.read(diagnosticRepositoryProvider);
    return widget.circuitId != null
        ? repo.speedTestCircuit(widget.circuitId!)
        : repo.speedTestNewCircuit(widget.hops);
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final title = widget.circuitId != null
        ? l10n.speedTestCircuitTitle(widget.circuitId!)
        : l10n.speedTestNewTitle(widget.hops);
    return AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 320,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (_error != null)
              Text(
                _error!,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              )
            else if (_last != null) ...[
              _rate(theme, Icons.arrow_upward, l10n.cardUpload, _last!.up),
              const SizedBox(height: AppSpace.sm),
              _rate(
                theme,
                Icons.arrow_downward,
                l10n.cardDownload,
                _last!.down,
              ),
            ] else
              Text(l10n.speedMeasuring),
            const SizedBox(height: AppSpace.md),
            if (!_done && _error == null)
              const LinearProgressIndicator()
            else
              Text(
                _error != null ? l10n.speedTestFailed : l10n.speedTestDone,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(l10n.close),
        ),
      ],
    );
  }

  Widget _rate(ThemeData theme, IconData icon, String label, double v) =>
      Row(
        children: [
          Icon(icon, size: 18),
          const SizedBox(width: AppSpace.sm),
          Expanded(child: Text(label)),
          Text(
            '${v.toStringAsFixed(2)} MiB/s',
            style: theme.textTheme.titleMedium,
          ),
        ],
      );
}
