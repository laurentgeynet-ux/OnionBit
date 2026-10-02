// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
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
                ? context.l10n.newVersionAvail('${r['new_version']}')
                : context.l10n.upToDate,
          ),
        ),
      );
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(context.l10n.checkError('$e'))),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final versions = ref.watch(versionsProvider);
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.system_update_alt,
      title: l10n.sectionVersioning,
      child: (context, settings) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            versions.when(
              loading: () => const LinearProgressIndicator(),
              error: (e, _) => ErrorState(
                error: e,
                onRetry: () => ref.invalidate(versionsProvider),
              ),
              data: (v) => Padding(
                padding: const EdgeInsets.only(bottom: AppSpacing.sm),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        l10n.daemonVersionLine('${v['current'] ?? '—'}'),
                        style: theme.textTheme.bodyMedium,
                      ),
                    ),
                    TextButton.icon(
                      icon: const Icon(Icons.refresh, size: 16),
                      label: Text(l10n.checkVersion),
                      onPressed: () => _check(context, ref),
                    ),
                  ],
                ),
              ),
            ),
            SettingsSwitch(
              path: const ['versioning', 'enabled'],
              value: settingsBool(settings, const [
                'versioning',
                'enabled',
              ], def: true),
              title: l10n.versionCheckSwitch,
              subtitle: l10n.versionCheckSub,
            ),
            SettingsSwitch(
              path: const ['versioning', 'allow_pre'],
              value: settingsBool(settings, const ['versioning', 'allow_pre']),
              title: l10n.allowPre,
              subtitle: l10n.defaultOff,
            ),
          ],
        );
      },
    );
  }
}
