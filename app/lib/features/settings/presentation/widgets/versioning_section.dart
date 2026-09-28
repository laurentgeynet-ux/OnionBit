import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/error_state.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Section « Mises à jour » — version du daemon
/// (`GET /api/versioning/versions`), sonde de mise à jour
/// (`/versions/check`) et activation de la section `versioning/*`.
class VersioningSection extends ConsumerWidget {
  const VersioningSection({super.key});

  Future<void> _check(BuildContext context, WidgetRef ref) async {
    try {
      final r = await ref.read(settingsRepositoryProvider).checkVersion();
      if (!context.mounted) return;
      final has = r['has_version'] == true;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            has
                ? 'Nouvelle version disponible : ${r['new_version']}'
                : 'Le daemon est à jour.',
          ),
        ),
      );
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Vérification : $e')));
      }
    }
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final versions = ref.watch(versionsProvider);
    return SettingsSection(
      icon: Icons.system_update_alt,
      title: 'Mises à jour',
      child: (context, settings) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            versions.when(
              loading: () => const LinearProgressIndicator(),
              error: (e, _) => ErrorState(
                message: '$e',
                onRetry: () => ref.invalidate(versionsProvider),
              ),
              data: (v) => Padding(
                padding: const EdgeInsets.only(bottom: AppSpacing.sm),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Version du daemon : ${v['current'] ?? '—'}',
                        style: theme.textTheme.bodyMedium,
                      ),
                    ),
                    TextButton.icon(
                      icon: const Icon(Icons.refresh, size: 16),
                      label: const Text('Vérifier'),
                      onPressed: () => _check(context, ref),
                    ),
                  ],
                ),
              ),
            ),
            SettingsSwitch(
              path: const ['versioning', 'enabled'],
              value: settingsBool(
                settings,
                const ['versioning', 'enabled'],
                def: true,
              ),
              title: 'Vérification de version',
              subtitle: 'Section désactivée = endpoints absents (404).',
            ),
            SettingsSwitch(
              path: const ['versioning', 'allow_pre'],
              value: settingsBool(
                settings,
                const ['versioning', 'allow_pre'],
              ),
              title: 'Accepter les pré-versions',
            ),
          ],
        );
      },
    );
  }
}
