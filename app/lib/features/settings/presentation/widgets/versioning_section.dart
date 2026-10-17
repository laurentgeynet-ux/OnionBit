// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/widgets/error_state.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Section « Version » — version du daemon
/// (`GET /api/versioning/versions`) et activation de la section
/// `versioning/*`.
///
/// Pas de sonde de mise à jour (décision vie privée) : interroger un
/// serveur de releases depuis le poste de l'utilisateur fuiterait
/// l'existence de l'installation — le bouton « Vérifier » et les
/// clés `check_urls`/`github_repo`/`allow_pre` ont été retirés.
class VersioningSection extends ConsumerWidget {
  const VersioningSection({super.key});

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
                padding: const EdgeInsets.only(bottom: AppSpace.sm),
                child: Text(
                  l10n.daemonVersionLine('${v['current'] ?? '—'}'),
                  style: theme.textTheme.bodyMedium,
                ),
              ),
            ),
            SettingsSwitch(
              path: const ['versioning', 'enabled'],
              value: settingsBool(settings, const [
                'versioning',
                'enabled',
              ], def: true),
              title: l10n.versionEndpointsSwitch,
              subtitle: l10n.versionEndpointsSub,
              defaultValue: true,
            ),
          ],
        );
      },
    );
  }
}
