import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
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
    final title = widget.circuitId != null
        ? 'Test du circuit #${widget.circuitId}'
        : 'Test d\'un circuit à ${widget.hops} saut(s)';
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
              _rate(theme, Icons.arrow_upward, 'Envoi', _last!.up),
              const SizedBox(height: AppSpacing.sm),
              _rate(
                theme,
                Icons.arrow_downward,
                'Réception',
                _last!.down,
              ),
            ] else
              const Text('Mesure en cours…'),
            const SizedBox(height: AppSpacing.md),
            if (!_done && _error == null)
              const LinearProgressIndicator()
            else
              Text(
                _error != null ? 'Échec du test' : 'Test terminé',
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
          child: const Text('Fermer'),
        ),
      ],
    );
  }

  Widget _rate(ThemeData theme, IconData icon, String label, double v) =>
      Row(
        children: [
          Icon(icon, size: 18),
          const SizedBox(width: AppSpacing.sm),
          Expanded(child: Text(label)),
          Text(
            '${v.toStringAsFixed(2)} MiB/s',
            style: theme.textTheme.titleMedium,
          ),
        ],
      );
}
