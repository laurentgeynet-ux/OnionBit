// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../providers/settings_providers.dart';
import '../settings_catalog.dart';
import '../widgets/anonymity_section.dart';
import '../widgets/appearance_section.dart';
import '../widgets/automation_section.dart';
import '../widgets/bandwidth_section.dart';
import '../widgets/connection_section.dart';
import '../widgets/daemon_section.dart';
import '../widgets/downloads_section.dart';
import '../widgets/identity_section.dart';
import '../widgets/network_section.dart';
import '../widgets/onionbit_section.dart';
import '../widgets/queue_section.dart';
import '../widgets/seeding_section.dart';
import '../widgets/stealth_section.dart';
import '../widgets/storage_section.dart';
import '../widgets/versioning_section.dart';

/// Entrée du catalogue des sections — id de l'ancre ; les mots-clés
/// de recherche vivent dans `settings_catalog.dart`
/// (`settingsSectionKeywords`), partagés avec la palette (ADR-0021 §6).
class _SectionEntry {
  _SectionEntry({required this.id, required this.child, this.sectionId});

  final SettingsSectionId id;
  final Widget child;

  /// Identifiant `settingsDirtyProvider` quand la section supporte la
  /// sauvegarde différée (null = enregistrement immédiat uniquement).
  final String? sectionId;
  final key = GlobalKey();
}

final _kSections = <_SectionEntry>[
  _SectionEntry(
    id: SettingsSectionId.appearance,
    child: const AppearanceSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.downloads,
    sectionId: 'downloads',
    child: const DownloadsSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.storage,
    child: const StorageSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.bandwidth,
    sectionId: 'bandwidth',
    child: const BandwidthSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.queue,
    sectionId: 'queue',
    child: const QueueSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.seeding,
    sectionId: 'seeding',
    child: const SeedingSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.anonymity,
    sectionId: 'anonymity',
    child: const AnonymitySection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.onionbit,
    sectionId: 'onionbit',
    child: const OnionBitSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.stealth,
    sectionId: 'stealth',
    child: const StealthSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.identity,
    sectionId: 'identity',
    child: const IdentitySection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.network,
    sectionId: 'network',
    child: const NetworkSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.automation,
    sectionId: 'automation',
    child: const AutomationSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.versioning,
    child: const VersioningSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.connection,
    child: const ConnectionSection(),
  ),
  _SectionEntry(
    id: SettingsSectionId.daemon,
    child: const DaemonSection(),
  ),
];

/// Page « Réglages » — rail d'ancres + filtre + sections du catalogue.
/// `?s=<id>` (deep-link palette de commandes, ADR-0021 §6) défile
/// jusqu'à la section demandée après la première frame.
class SettingsPage extends ConsumerStatefulWidget {
  const SettingsPage({super.key, this.sectionId});

  /// Ancre ciblée par `/settings?s=<name>` (`SettingsSectionId.name`) —
  /// `null` hors deep-link ; une valeur inconnue est ignorée.
  final String? sectionId;

  @override
  ConsumerState<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends ConsumerState<SettingsPage> {
  String _filter = '';
  bool _savingAll = false;
  String? _scrolledTo;

  /// Défile jusqu'à la section `?s=` demandée — post-frame (les
  /// `GlobalKey` des entrées n'existent qu'après le premier layout) et
  /// une fois par valeur (le paramètre peut changer sans recréer la
  /// page, ex. deux commandes réglages d'affilée).
  void _scrollToRequested() {
    final wanted = widget.sectionId;
    if (wanted == null || wanted == _scrolledTo) return;
    _scrolledTo = wanted;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (_filter.isNotEmpty) {
        // Un filtre actif masquerait la section : le deep-link est une
        // intention de navigation — liste pleine d'abord, défilement à
        // la frame suivante (l'ancre n'existe qu'une fois rendue).
        setState(() => _filter = '');
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) _doScroll(wanted);
        });
        return;
      }
      _doScroll(wanted);
    });
  }

  void _doScroll(String wanted) {
    for (final e in _kSections) {
      if (e.id.name == wanted) {
        final ctx = e.key.currentContext;
        if (ctx != null) {
          Scrollable.ensureVisible(
            ctx,
            duration: const Duration(milliseconds: 250),
          );
        }
        break;
      }
    }
  }

  Future<void> _saveAll() async {
    setState(() => _savingAll = true);
    // Chaque `_save` se marque propre en cas de succès ; les sections en
    // échec conservent leur pastille « modifié ».
    for (final entry in ref.read(settingsSaveBusProvider).values) {
      await entry.save();
    }
    if (mounted) setState(() => _savingAll = false);
  }

  void _discardAll() {
    for (final entry in ref.read(settingsSaveBusProvider).values) {
      entry.discard();
    }
    ref.read(settingsDirtyProvider.notifier).clear();
  }

  bool _matches(_SectionEntry e, AppLocalizations l10n) =>
      _filter.isEmpty ||
      '${e.id.title(l10n)} ${settingsSectionKeywords[e.id] ?? ''}'
          .toLowerCase()
          .contains(_filter.toLowerCase());

  @override
  Widget build(BuildContext context) {
    _scrollToRequested();
    final l10n = context.l10n;
    final visible = _kSections.where((e) => _matches(e, l10n)).toList();
    final dirty = ref.watch(settingsDirtyProvider);
    final scheme = Theme.of(context).colorScheme;
    return Column(
      children: [
        if (dirty.isNotEmpty)
          Material(
            color: scheme.tertiaryContainer,
            child: Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: AppSpace.md,
                vertical: AppSpace.xs,
              ),
              child: Row(
                children: [
                  Icon(
                    Icons.edit_note,
                    size: 20,
                    color: scheme.onTertiaryContainer,
                  ),
                  const SizedBox(width: AppSpace.sm),
                  Expanded(
                    child: Text(
                      l10n.settingsDirtyBanner(dirty.length),
                      style: Theme.of(context).textTheme.bodySmall
                          ?.copyWith(color: scheme.onTertiaryContainer),
                    ),
                  ),
                  TextButton(
                    onPressed: _discardAll,
                    child: Text(l10n.discardAll),
                  ),
                  const SizedBox(width: AppSpace.xs),
                  FilledButton.icon(
                    onPressed: _savingAll ? null : _saveAll,
                    icon: _savingAll
                        ? const SizedBox(
                            width: 14,
                            height: 14,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.save, size: 18),
                    label: Text(l10n.saveAll),
                  ),
                ],
              ),
            ),
          ),
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpace.md,
            vertical: AppSpace.xs,
          ),
          child: Row(
            children: [
              Expanded(
                // Rail d'ancres — une chip par section visible.
                child: SizedBox(
                  height: 34,
                  child: ListView(
                    scrollDirection: Axis.horizontal,
                    children: [
                      for (final e in visible)
                        Padding(
                          padding: const EdgeInsets.only(right: AppSpace.xs),
                          child: ActionChip(
                            avatar:
                                e.sectionId != null &&
                                    dirty.contains(e.sectionId)
                                ? Icon(
                                    Icons.circle,
                                    size: 8,
                                    color: scheme.tertiary,
                                  )
                                : null,
                            label: Text(e.id.title(l10n)),
                            visualDensity: VisualDensity.compact,
                            onPressed: () {
                              final ctx = e.key.currentContext;
                              if (ctx != null) {
                                Scrollable.ensureVisible(
                                  ctx,
                                  duration: const Duration(milliseconds: 250),
                                );
                              }
                            },
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: AppSpace.sm),
              SizedBox(
                width: 220,
                child: TextField(
                  decoration: InputDecoration(
                    hintText: l10n.filterSettings,
                    isDense: true,
                    prefixIcon: const Icon(Icons.filter_list, size: 18),
                    border: const OutlineInputBorder(),
                  ),
                  onChanged: (v) => setState(() => _filter = v),
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: ListView(
            children: [
              for (final e in visible) KeyedSubtree(key: e.key, child: e.child),
              if (visible.isEmpty)
                Padding(
                  padding: const EdgeInsets.all(AppSpace.lg),
                  child: Center(child: Text(l10n.noMatchingSection(_filter))),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
