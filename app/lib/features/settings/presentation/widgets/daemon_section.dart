import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';

/// Section « Daemon » — résumé de la configuration effective
/// (`GET /api/settings`) + arrêt (`PUT /api/shutdown`).
class DaemonSection extends ConsumerWidget {
  const DaemonSection({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final settings = ref.watch(daemonSettingsProvider);

    return Card(
      margin: const EdgeInsets.all(AppSpacing.md),
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.md),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
            children: [
              Expanded(
                child: Text(
                  'État du daemon',
                  style: theme.textTheme.titleMedium,
                ),
              ),
              IconButton(
                tooltip: 'Rafraîchir',
                onPressed: () => ref.invalidate(daemonSettingsProvider),
                icon: const Icon(Icons.refresh, size: 18),
              ),
            ],
          ),
          settings.when(
            loading: () => const Padding(
              padding: EdgeInsets.all(AppSpacing.sm),
              child: LinearProgressIndicator(),
            ),
            error: (e, _) => Text(
              '$e',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.error,
              ),
            ),
            data: (s) {
              final lt = s['libtorrent'] as Map<String, dynamic>?;
              final defaults =
                  lt?['download_defaults'] as Map<String, dynamic>?;
              final tunnel = s['tunnel_community'] as Map<String, dynamic>?;
              final ipv8 = s['ipv8'] as Map<String, dynamic>?;
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _line(
                    context,
                    'Dossier d\'état',
                    '${s['state_dir'] ?? '—'}',
                  ),
                  _line(
                    context,
                    'Téléchargements vers',
                    '${defaults?['saveas'] ?? '—'}',
                  ),
                  _line(
                    context,
                    'IPv8',
                    (ipv8?['enabled'] == true)
                        ? 'actif (${ipv8?['address'] ?? ''})'
                        : 'inactif',
                  ),
                  _line(
                    context,
                    'Tunnels anonymes',
                    (tunnel?['enabled'] == true) ? 'activés' : 'désactivés',
                  ),
                ],
              );
            },
          ),
          const SizedBox(height: AppSpacing.sm),
          Align(
            alignment: Alignment.centerRight,
            child: OutlinedButton.icon(
              icon: const Icon(Icons.power_settings_new, size: 18),
              label: const Text('Arrêter le daemon'),
              onPressed: () => _confirmShutdown(context, ref),
            ),
          ),
        ],
      ),
    ),
  );
  }

  Widget _line(BuildContext context, String label, String value) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 160,
            child: Text(
              label,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          Expanded(child: Text(value, style: theme.textTheme.bodySmall)),
        ],
      ),
    );
  }

  Future<void> _confirmShutdown(BuildContext context, WidgetRef ref) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Arrêter le daemon ?'),
        content: const Text(
          'Les téléchargements seront suspendus jusqu\'au prochain '
          'démarrage.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Annuler'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Arrêter'),
          ),
        ],
      ),
    );
    if (ok == true && context.mounted) {
      try {
        await ref.read(settingsRepositoryProvider).shutdown();
      } catch (_) {
        // Le daemon coupe la connexion avant/après la réponse — un
        // échec de la requête ne signifie pas que l'arrêt a échoué.
      }
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Arrêt du daemon demandé.')),
        );
      }
    }
  }
}
