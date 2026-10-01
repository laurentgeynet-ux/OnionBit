// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Section « Téléchargements & Anonymat » — réglages du dossier par défaut et comportement réseau.
class DownloadsSection extends ConsumerStatefulWidget {
  const DownloadsSection({super.key});

  @override
  ConsumerState<DownloadsSection> createState() => _DownloadsSectionState();
}

class _DownloadsSectionState extends ConsumerState<DownloadsSection> {
  late final _deferred = DeferredSection(ref, 'downloads');
  final _saveasController = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
  }

  @override
  void dispose() {
    _deferred.detach();
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
      setState(() {
        _saveasController.text = dir;
        _deferred.markDirty();
      });
    }
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    final repo = ref.read(settingsRepositoryProvider);
    final saveas = _saveasController.text.trim();
    try {
      await repo.update({
        'libtorrent': {
          'download_defaults': {'saveas': saveas},
        },
      });
      ref.invalidate(daemonSettingsProvider);
      _deferred.markClean();
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(context.l10n.dlSettingsSaved)),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(context.l10n.saveError('$e'))),
        );
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
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
                    l10n.sectionDownloadsDefaults,
                    style: theme.textTheme.titleMedium,
                  ),
                ),
                if (ref.watch(
                  settingsDirtyProvider.select((s) => s.contains('downloads')),
                ))
                  Tooltip(
                    message: l10n.unsavedChanges,
                    child: Icon(
                      Icons.circle,
                      size: 10,
                      color: theme.colorScheme.tertiary,
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
              error: (e, _) => Text(l10n.errorMessage('$e')),
              data: (settings) {
                _syncWithSettings(settings);
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    TextField(
                      controller: _saveasController,
                      onChanged: (_) => _deferred.markDirty(),
                      decoration: InputDecoration(
                        labelText: l10n.dlDestFolder,
                        hintText: l10n.dlDestHint,
                        prefixIcon: const Icon(Icons.folder_outlined),
                        suffixIcon: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            KeyInfoIcon(const [
                              'libtorrent',
                              'download_defaults',
                              'saveas',
                            ]),
                            IconButton(
                              tooltip: l10n.browse,
                              icon: const Icon(Icons.folder_open),
                              onPressed: _pickDirectory,
                            ),
                          ],
                        ),
                      ),
                    ),
                    const SizedBox(height: AppSpacing.xs),
                    Text(
                      l10n.dlDestHelp,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                    const SizedBox(height: AppSpacing.sm),
                    _DiskSpace(directory: _saveasController.text.trim()),
                    const SizedBox(height: AppSpacing.md),
                    Align(
                      alignment: Alignment.centerRight,
                      child: FilledButton.icon(
                        onPressed: _saving ? null : _save,
                        icon: const Icon(Icons.save),
                        label: Text(l10n.save),
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

/// Ligne « Espace disque » du dossier de destination
/// (`PUT /api/statistics/dirspace` — le backend mesure le premier
/// ancêtre existant, comme `shutil.disk_usage` Python).
class _DiskSpace extends ConsumerWidget {
  const _DiskSpace({required this.directory});

  final String directory;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final space = ref.watch(
      dirSpaceProvider(directory.isEmpty ? null : directory),
    );
    final style = Theme.of(context).textTheme.bodySmall;
    return space.when(
      loading: () => Text(context.l10n.diskSpaceLoading, style: style),
      error: (_, _) =>
          Text(context.l10n.diskSpaceUnavailable, style: style),
      data: (s) => Row(
        children: [
          Icon(
            Icons.storage,
            size: 14,
            color: Theme.of(context).colorScheme.outline,
          ),
          const SizedBox(width: AppSpacing.xs),
          Expanded(
            child: Text(
              context.l10n.diskSpaceFree(
                ByteFormatter.format(s['free'] ?? 0),
                ByteFormatter.format(s['total'] ?? 0),
              ),
              style: style,
            ),
          ),
        ],
      ),
    );
  }
}
