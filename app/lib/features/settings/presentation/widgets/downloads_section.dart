import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';

/// Section « Téléchargements & Anonymat » — réglages du dossier par défaut et comportement réseau.
class DownloadsSection extends ConsumerStatefulWidget {
  const DownloadsSection({super.key});

  @override
  ConsumerState<DownloadsSection> createState() => _DownloadsSectionState();
}

class _DownloadsSectionState extends ConsumerState<DownloadsSection> {
  final _saveasController = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void dispose() {
    _saveasController.dispose();
    super.dispose();
  }

  void _syncWithSettings(Map<String, dynamic> settings) {
    if (_initialized) return;
    final lt = settings['libtorrent'] as Map<String, dynamic>?;
    final dd = lt?['download_defaults'] as Map<String, dynamic>?;
    final saveas = (dd?['saveas'] as String?) ?? '';
    _saveasController.text = saveas;
    _initialized = true;
  }

  Future<void> _pickDirectory() async {
    final dir = await getDirectoryPath();
    if (dir != null) {
      setState(() => _saveasController.text = dir);
    }
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    final repo = ref.read(settingsRepositoryProvider);
    final saveas = _saveasController.text.trim();
    try {
      await repo.update({
        'libtorrent': {
          'download_defaults': {
            'saveas': saveas,
          },
        },
      });
      ref.invalidate(daemonSettingsProvider);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Réglages de téléchargement enregistrés')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Erreur lors de l\'enregistrement : $e')),
        );
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final async = ref.watch(daemonSettingsProvider);

    return Card(
      margin: const EdgeInsets.symmetric(
        horizontal: AppSpacing.md,
        vertical: AppSpacing.sm,
      ),
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.md),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                const Icon(Icons.download),
                const SizedBox(width: AppSpacing.sm),
                Expanded(
                  child: Text(
                    'Téléchargements par défaut',
                    style: theme.textTheme.titleMedium,
                  ),
                ),
                if (_saving)
                  const SizedBox(
                    width: 16,
                    height: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
              ],
            ),
            const SizedBox(height: AppSpacing.md),
            async.when(
              loading: () => const Center(child: CircularProgressIndicator()),
              error: (e, _) => Text('Erreur : $e'),
              data: (settings) {
                _syncWithSettings(settings);
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    TextField(
                      controller: _saveasController,
                      decoration: InputDecoration(
                        labelText: 'Dossier de destination par défaut',
                        hintText: 'C:\\Users\\...\\Downloads',
                        prefixIcon: const Icon(Icons.folder_outlined),
                        suffixIcon: IconButton(
                          tooltip: 'Parcourir…',
                          icon: const Icon(Icons.folder_open),
                          onPressed: _pickDirectory,
                        ),
                      ),
                    ),
                    const SizedBox(height: AppSpacing.xs),
                    Text(
                      'Les nouveaux téléchargements sans dossier spécifique seront enregistrés ici.',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                    const SizedBox(height: AppSpacing.md),
                    Align(
                      alignment: Alignment.centerRight,
                      child: FilledButton.icon(
                        onPressed: _saving ? null : _save,
                        icon: const Icon(Icons.save),
                        label: const Text('Enregistrer'),
                      ),
                    ),
                  ],
                );
              },
            ),
          ],
        ),
      ),
    );
  }
}
