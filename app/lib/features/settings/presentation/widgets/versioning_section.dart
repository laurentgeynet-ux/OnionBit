// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart' show ApiException;
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
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
            const Divider(height: AppSpace.lg),
            // Base locale (ADR-0031) : `database.encrypted` reflète
            // le codec sqlite3mc appliqué à l'ouverture — indication
            // honnête lue du réglage effectif, jamais déduite.
            if (settingsBool(settings, const ['database', 'encrypted'],
                def: true))
              Padding(
                padding: const EdgeInsets.only(bottom: AppSpace.sm),
                child: Chip(
                  avatar: const Icon(Icons.lock_outline, size: 16),
                  label: Text(l10n.dbEncrypted),
                  visualDensity: VisualDensity.compact,
                ),
              ),
            const Divider(height: AppSpace.lg),
            const _ObupBlock(),
          ],
        );
      },
    );
  }
}

/// Bloc « mise à jour signée (OBUP) » (ADR-0032) : import manuel du
/// bundle déposé dans `state/update/incoming/` + statut du staging.
/// Aucun upload : l'API vérifie la signature avant de stager, et le
/// launcher re-vérifie au swap — l'UI affiche l'état honnête.
class _ObupBlock extends ConsumerWidget {
  const _ObupBlock();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final status = ref.watch(updateStatusProvider);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(l10n.updateTitle, style: theme.textTheme.labelMedium),
        const SizedBox(height: AppSpace.xs),
        Text(
          l10n.updateHint,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: AppSpace.sm),
        status.when(
          loading: () => const SizedBox.shrink(),
          error: (_, _) => const SizedBox.shrink(),
          data: (s) => Padding(
            padding: const EdgeInsets.only(bottom: AppSpace.sm),
            child: Text(
              s['staged'] == true
                  ? l10n.updateStaged(
                      '${s['version'] ?? '—'}',
                      (s['files'] as num?)?.toInt() ?? 0,
                    )
                  : s['invalid'] == true
                  ? l10n.updateInvalid
                  : l10n.updateNone,
              style: theme.textTheme.bodySmall,
            ),
          ),
        ),
        FilledButton.tonalIcon(
          icon: const Icon(Icons.system_update_alt, size: 18),
          label: Text(l10n.updateImportButton),
          onPressed: () => _import(context, ref),
        ),
      ],
    );
  }

  /// `POST /api/versioning/import` — codes `obup_*` traduits, jamais
  /// le message brut (la cause exacte reste dans les logs daemon).
  Future<void> _import(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final messenger = ScaffoldMessenger.of(context);
    try {
      final r = await ref.read(settingsRepositoryProvider).importUpdate();
      ref.invalidate(updateStatusProvider);
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            l10n.updateStaged(
              '${r['version'] ?? '—'}',
              (r['files'] as num?)?.toInt() ?? 0,
            ),
          ),
        ),
      );
    } on ApiException catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text(_obupErrorLabel(l10n, e))),
      );
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text(l10n.updateErrGeneric('$e'))),
      );
    }
  }

  /// Libellé localisé d'un échec d'import — le code machine
  /// `obup_*` (ADR-0032) prime ; 404 sans artefact = « déposez le
  /// bundle ».
  String _obupErrorLabel(AppLocalizations l10n, ApiException e) =>
      switch (e.code) {
        'obup_malformed' || 'obup_bad_path' => l10n.updateErrMalformed,
        'obup_sig_invalid' => l10n.updateErrSig,
        'obup_unknown_key' => l10n.updateErrKey,
        'obup_too_large' => l10n.updateErrLarge,
        'obup_version_too_old' || 'obup_min_supported' =>
          l10n.updateErrTooOld,
        'obup_missing_file' || 'obup_hash_mismatch' =>
          l10n.updateErrBadFile,
        _ => e.statusCode == 404
            ? l10n.updateErrMissing
            : l10n.updateErrGeneric(e.message),
      };
}
