// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../l10n/app_localizations.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Section « Stockage » (ADR-0018) — zones public/privé, rangement à
/// complétion (`storage/move_on_completion`), zone par défaut,
/// état de la zone privée (`GET /api/private`), espace disque par
/// zone et signalement des chemins non portables (valeurs persistées
/// hors specs `@state`/`@public`/`@private`).
class StorageSection extends ConsumerWidget {
  const StorageSection({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.storage,
      title: l10n.sectionStorage,
      child: (context, settings) => _body(context, ref, settings, l10n),
    );
  }

  Widget _body(
    BuildContext context,
    WidgetRef ref,
    Map<String, dynamic> settings,
    AppLocalizations l10n,
  ) {
    final theme = Theme.of(context);
    final storage = settings['storage'];
    final moveOnCompletion =
        storage is Map && storage['move_on_completion'] == true;
    final privateEnabled =
        storage is! Map || storage['private_enabled'] != false;
    final defaultArea = storage is Map
        ? '${storage['default_area'] ?? 'public'}'
        : 'public';
    final privateAsync = ref.watch(privateZoneProvider);
    final privateState = privateAsync.value?['state'] as String? ?? 'locked';
    // Chemins persistés non portables : les valeurs valides sont
    // vides ou des specs `@root/…`/`@public/…`/`@private/…` — un
    // chemin absolu survivrait mal à un déplacement du bundle (clé
    // USB : la lettre de lecteur change d'un hôte à l'autre).
    final saveas = settingsString(settings, const [
      'libtorrent',
      'download_defaults',
      'saveas',
    ]);
    final nonPortable = saveas.isNotEmpty && !saveas.startsWith('@');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Zone par défaut des nouveaux ajouts.
        Row(
          children: [
            Expanded(
              child: Text(
                l10n.storageDefaultArea,
                style: theme.textTheme.bodyMedium,
              ),
            ),
            KeyInfoIcon(const ['storage', 'default_area']),
            const SizedBox(width: AppSpace.sm),
            SegmentedButton<String>(
              segments: [
                ButtonSegment(
                  value: 'public',
                  label: Text(l10n.storageAreaPublic),
                ),
                ButtonSegment(
                  value: 'private',
                  enabled: privateEnabled,
                  label: Text(l10n.storageAreaPrivate),
                ),
              ],
              selected: {defaultArea == 'private' ? 'private' : 'public'},
              onSelectionChanged: (s) => applySettingsPatch(context, ref, {
                'storage': {'default_area': s.first},
              }),
            ),
          ],
        ),
        SettingsSwitch(
          path: const ['storage', 'move_on_completion'],
          value: moveOnCompletion,
          title: l10n.storageMoveOnCompletion,
          subtitle: l10n.storageMoveOnCompletionSub,
        ),
        SettingsSwitch(
          path: const ['storage', 'private_enabled'],
          value: privateEnabled,
          title: l10n.storagePrivateEnabled,
          subtitle: l10n.storagePrivateEnabledSub,
        ),
        if (nonPortable) ...[
          const SizedBox(height: AppSpace.sm),
          _WarningCard(
            icon: Icons.folder_off_outlined,
            title: l10n.storageNonPortableTitle,
            body: l10n.storageNonPortableBody(saveas),
          ),
        ],
        const SizedBox(height: AppSpace.sm),
        // État de la zone privée (manifeste OBM + orphelins).
        privateAsync.when(
          loading: () => const LinearProgressIndicator(),
          error: (_, _) => Text(l10n.diskSpaceUnavailable),
          data: (zone) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(
                    privateState == 'mounted'
                        ? Icons.lock
                        : privateState == 'guest'
                        ? Icons.person_off_outlined
                        : Icons.lock_open,
                    size: 16,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: AppSpace.xs),
                  Expanded(
                    child: Text(
                      l10n.storagePrivateState(
                        switch (privateState) {
                          'mounted' => l10n.storagePrivateStateMounted,
                          'guest' => l10n.storagePrivateStateGuest,
                          _ => l10n.storagePrivateStateLocked,
                        },
                      ),
                      style: theme.textTheme.bodySmall,
                    ),
                  ),
                ],
              ),
              // Orphelins `.obd`/`.bitv` détectés au montage : purge
              // explicite uniquement — jamais de suppression silencieuse.
              if (_orphans(zone) > 0) ...[
                const SizedBox(height: AppSpace.xs),
                _WarningCard(
                  icon: Icons.cleaning_services_outlined,
                  title: l10n.storageOrphansTitle,
                  body: l10n.storageOrphansBody(_orphans(zone)),
                  action: TextButton(
                    onPressed: () => _purgeOrphans(context, ref),
                    child: Text(l10n.storageOrphansPurge),
                  ),
                ),
              ],
            ],
          ),
        ),
        const SizedBox(height: AppSpace.sm),
        // Espace disque par zone (`PUT /api/statistics/dirspace`,
        // paramètre `area` ADR-0018).
        _ZoneSpace(area: 'public', label: l10n.storageAreaPublic),
        _ZoneSpace(area: 'private', label: l10n.storageAreaPrivate),
      ],
    );
  }

  static int _orphans(Map<String, dynamic> zone) {
    final o = zone['orphans'];
    if (o is! Map) return 0;
    return ((o['obd_groups'] as num?)?.toInt() ?? 0) +
        ((o['bitv'] as num?)?.toInt() ?? 0);
  }

  Future<void> _purgeOrphans(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.storageOrphansPurge),
        content: Text(l10n.storageOrphansPurgeConfirm),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.storageOrphansPurge),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref.read(settingsRepositoryProvider).purgePrivateOrphans();
      ref.invalidate(privateZoneProvider);
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.errorMessage('$e'))),
        );
      }
    }
  }
}

/// Carte d'avertissement réutilisable (chemin non portable, orphelins).
class _WarningCard extends StatelessWidget {
  const _WarningCard({
    required this.icon,
    required this.title,
    required this.body,
    this.action,
  });

  final IconData icon;
  final String title;
  final String body;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.all(AppSpace.sm),
      decoration: BoxDecoration(
        color: scheme.tertiaryContainer.withAlpha(120),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: scheme.tertiary),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 20, color: scheme.tertiary),
          const SizedBox(width: AppSpace.sm),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title, style: Theme.of(context).textTheme.labelLarge),
                const SizedBox(height: 2),
                Text(
                  body,
                  style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: scheme.onTertiaryContainer,
                  ),
                ),
                ?action,
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// Ligne « espace disque » d'une zone (`dirspace?area=`).
class _ZoneSpace extends ConsumerWidget {
  const _ZoneSpace({required this.area, required this.label});

  final String area;
  final String label;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final space = ref.watch(zoneSpaceProvider(area));
    final style = Theme.of(context).textTheme.bodySmall;
    return Row(
      children: [
        SizedBox(
          width: 110,
          child: Text(label, style: style),
        ),
        Expanded(
          child: space.when(
            loading: () => Text(context.l10n.diskSpaceLoading, style: style),
            error: (_, _) =>
                Text(context.l10n.diskSpaceUnavailable, style: style),
            data: (s) => Text(
              context.l10n.diskSpaceFree(
                context.fmtBytes(s['free'] ?? 0),
                context.fmtBytes(s['total'] ?? 0),
              ),
              style: style,
            ),
          ),
        ),
      ],
    );
  }
}
