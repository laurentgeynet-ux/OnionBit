// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/platform/pick_directory.dart';
import '../../../../core/platform/private_cache.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../domain/private_zone.dart';
import '../providers/private_zone_providers.dart';

/// Page « Zone privée » — explorateur du contenu chiffré de la zone
/// (ADR-0027, étape 110) : arborescence adossée au **manifeste**
/// (source de vérité, pas la liste des téléchargements), lecture
/// via export vers le cache temporaire, extraction en clair vers un
/// dossier choisi. Visible dans la nav uniquement quand
/// `state == 'mounted'` — la page elle-même garde un état
/// « verrouillée » au cas où la zone se referme en cours de route.
class PrivateZonePage extends ConsumerWidget {
  const PrivateZonePage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final catalog = ref.watch(privateZoneCatalogProvider);
    final l10n = context.l10n;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpace.md,
            vertical: AppSpace.sm,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  l10n.privateZoneTitle,
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
              IconButton(
                tooltip: l10n.refresh,
                onPressed: ref.read(privateZoneCatalogProvider.notifier).refresh,
                icon: const Icon(Icons.refresh, size: 20),
              ),
            ],
          ),
        ),
        Expanded(
          child: catalog.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              error: e,
              onRetry: () => ref.invalidate(privateZoneCatalogProvider),
            ),
            data: (c) => _ZoneView(catalog: c),
          ),
        ),
      ],
    );
  }
}

/// Corps de la page : état `mounted` requis — `locked`/`guest`
/// n'ont aucun catalogue lisible (le manifeste est chiffré).
class _ZoneView extends StatelessWidget {
  const _ZoneView({required this.catalog});

  final PrivateZoneCatalog catalog;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    if (catalog.state != 'mounted') {
      return EmptyState(
        icon: Icons.lock_outline,
        title: l10n.privateZoneLockedTitle,
        message: l10n.privateZoneLockedMessage,
      );
    }
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Les exports/lectures produisent des copies EN CLAIR — hors
        // périmètre chiffré, responsabilité de l'utilisateur
        // (ADR-0027 §Conséquences).
        Padding(
          padding: const EdgeInsets.fromLTRB(
            AppSpace.md,
            0,
            AppSpace.md,
            AppSpace.sm,
          ),
          child: Text(
            l10n.privateZoneInfo,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        Expanded(
          child: catalog.entries.isEmpty
              ? EmptyState(
                  icon: Icons.enhanced_encryption_outlined,
                  title: l10n.privateZoneEmptyTitle,
                  message: l10n.privateZoneEmptyMessage,
                )
              : ListView.builder(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpace.sm,
                  ),
                  itemCount: catalog.entries.length,
                  itemBuilder: (context, i) =>
                      _PrivateEntryCard(entry: catalog.entries[i]),
                ),
        ),
        if (catalog.orphanGroups > 0 || catalog.orphanBitv > 0)
          Padding(
            padding: const EdgeInsets.all(AppSpace.md),
            child: Text(
              l10n.privateZoneOrphans(
                catalog.orphanGroups,
                catalog.orphanBitv,
              ),
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
      ],
    );
  }
}

/// Entrée manifeste : titre (nom réel ou infohash tronqué quand
/// reconstruite), sous-zone et date, menu « Extraire vers… »,
/// fichiers chargés à l'expansion.
class _PrivateEntryCard extends ConsumerWidget {
  const _PrivateEntryCard({required this.entry});

  final PrivateEntry entry;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    final title = entry.name.isEmpty
        ? '${entry.infohash.substring(0, 12.clamp(0, entry.infohash.length))}…'
        : entry.name;
    return Card(
      margin: const EdgeInsets.only(bottom: AppSpace.sm),
      clipBehavior: Clip.antiAlias,
      child: ExpansionTile(
        leading: const Icon(Icons.folder_zip_outlined),
        title: Text(title, overflow: TextOverflow.ellipsis),
        subtitle: Text(
          [
            entry.destination,
            if (entry.name.isEmpty) l10n.privateZoneReconstructed,
            if (entry.timeAdded > 0)
              MaterialLocalizations.of(context).formatShortDate(
                DateTime.fromMillisecondsSinceEpoch(entry.timeAdded * 1000),
              ),
          ].join(' · '),
          style: theme.textTheme.bodySmall,
        ),
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            _EntryActions(entry: entry),
            const Icon(Icons.expand_more, size: 20),
          ],
        ),
        children: [
          const Divider(height: 1),
          _EntryFiles(entry: entry),
        ],
      ),
    );
  }
}

/// Menu contextuel d'une entrée entière — « Extraire vers un
/// dossier… » exporte tous ses fichiers.
class _EntryActions extends ConsumerWidget {
  const _EntryActions({required this.entry});

  final PrivateEntry entry;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    return PopupMenuButton<String>(
      tooltip: l10n.privateZoneActions,
      icon: const Icon(Icons.more_vert, size: 20),
      itemBuilder: (context) => [
        PopupMenuItem(
          value: 'extract',
          child: ListTile(
            dense: true,
            contentPadding: EdgeInsets.zero,
            leading: const Icon(Icons.output, size: 18),
            title: Text(l10n.privateZoneExtract),
          ),
        ),
      ],
      onSelected: (_) => _exportAll(context, ref),
    );
  }

  Future<void> _exportAll(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final dest = await pickDaemonDirectory(context);
    if (dest == null || !context.mounted) return;
    try {
      final out = await ref
          .read(privateZoneRepositoryProvider)
          .export(entry.infohash, dest);
      if (!context.mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(l10n.privateZoneExported(out.exported, out.bytes))),
      );
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(l10n.privateZoneError('$e'))));
    }
  }
}

/// Fichiers d'une entrée — `GET /api/private/{key}/files` au
/// premier dépliage ; chaque ligne a « Lire » (export-cache +
/// ouverture) et « Extraire vers… ».
class _EntryFiles extends ConsumerWidget {
  const _EntryFiles({required this.entry});

  final PrivateEntry entry;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final files = ref.watch(privateFilesProvider(entry.infohash));
    return files.when(
      loading: () => const Padding(
        padding: EdgeInsets.all(AppSpace.md),
        child: Center(child: CircularProgressIndicator()),
      ),
      error: (e, _) => Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: ErrorState(
          error: e,
          onRetry: () => ref.invalidate(privateFilesProvider(entry.infohash)),
        ),
      ),
      data: (list) => Column(
        children: [
          for (final f in list) _FileTile(entry: entry, file: f),
        ],
      ),
    );
  }
}

class _FileTile extends ConsumerWidget {
  const _FileTile({required this.entry, required this.file});

  final PrivateEntry entry;
  final PrivateFileEntry file;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    return ListTile(
      dense: true,
      leading: const Icon(Icons.insert_drive_file_outlined, size: 20),
      title: Text(file.path, overflow: TextOverflow.ellipsis),
      subtitle: Text(
        context.fmtBytes(file.length),
        style: Theme.of(context).textTheme.bodySmall,
      ),
      trailing: PopupMenuButton<String>(
        tooltip: l10n.privateZoneActions,
        icon: const Icon(Icons.more_vert, size: 18),
        itemBuilder: (context) => [
          // « Lire » = export vers le cache temporaire puis shell —
          // masquée hors desktop (pas de destination sûre sur web).
          if (supportsPrivateReadCache)
            PopupMenuItem(
              value: 'read',
              child: ListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                leading: const Icon(Icons.visibility_outlined, size: 18),
                title: Text(l10n.privateZoneRead),
              ),
            ),
          PopupMenuItem(
            value: 'extract',
            child: ListTile(
              dense: true,
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.output, size: 18),
              title: Text(l10n.privateZoneExtract),
            ),
          ),
        ],
        onSelected: (v) => switch (v) {
          'read' => _read(context, ref),
          _ => _extract(context, ref),
        },
      ),
      onTap: supportsPrivateReadCache ? () => _read(context, ref) : null,
    );
  }

  /// « Lire » : copie déchiffrée vers un dossier temporaire de
  /// l'app puis `openPath` — la copie n'est pas chiffrée (étiquetée
  /// à l'affichage par l'info de page).
  Future<void> _read(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final dir = await privateReadCacheDir();
    if (dir == null || !context.mounted) return;
    try {
      await ref
          .read(privateZoneRepositoryProvider)
          .export(entry.infohash, dir, files: [file.index]);
      if (!context.mounted) return;
      await openPath(joinCachePath(dir, file.path));
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(l10n.privateZoneError('$e'))));
    }
  }

  /// « Extraire vers un dossier… » : destination choisie → export
  /// du fichier seul → toast (compte + octets, jamais de nom).
  Future<void> _extract(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final dest = await pickDaemonDirectory(context);
    if (dest == null || !context.mounted) return;
    try {
      final out = await ref
          .read(privateZoneRepositoryProvider)
          .export(entry.infohash, dest, files: [file.index]);
      if (!context.mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(l10n.privateZoneExported(out.exported, out.bytes))),
      );
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(l10n.privateZoneError('$e'))));
    }
  }
}
