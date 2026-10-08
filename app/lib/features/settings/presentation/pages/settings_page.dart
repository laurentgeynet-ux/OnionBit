// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../providers/settings_providers.dart';
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

/// Identifiants des sections de réglages — les titres affichés sont
/// localisés via [_SectionIdX.title] (ARB).
enum _SectionId {
  appearance,
  downloads,
  storage,
  bandwidth,
  queue,
  seeding,
  anonymity,
  onionbit,
  stealth,
  identity,
  network,
  automation,
  versioning,
  connection,
  daemon,
}

extension on _SectionId {
  /// Titre localisé de la section (puce d'ancre + filtre).
  String title(AppLocalizations l10n) => switch (this) {
    _SectionId.appearance => l10n.settingsAppearanceTitle,
    _SectionId.downloads => l10n.sectionDownloadsDefaults,
    _SectionId.storage => l10n.sectionStorage,
    _SectionId.bandwidth => l10n.sectionBandwidth,
    _SectionId.queue => l10n.sectionQueue,
    _SectionId.seeding => l10n.sectionSeeding,
    _SectionId.anonymity => l10n.sectionAnonymity,
    _SectionId.onionbit => l10n.sectionOnionBit,
    _SectionId.stealth => l10n.sectionStealth,
    _SectionId.identity => l10n.sectionIdentity,
    _SectionId.network => l10n.sectionNetwork,
    _SectionId.automation => l10n.sectionAutomation,
    _SectionId.versioning => l10n.sectionVersioning,
    _SectionId.connection => l10n.sectionConnection,
    _SectionId.daemon => l10n.sectionDaemon,
  };
}

/// Entrée du catalogue des sections — id de l'ancre + mots-clés de
/// recherche (chemins de clés + termes FR/EN, non affichés).
class _SectionEntry {
  _SectionEntry({
    required this.id,
    required this.keywords,
    required this.child,
    this.sectionId,
  });

  final _SectionId id;

  /// Texte de recherche : termes FR+EN + noms de champs + chemins de
  /// clés (jamais affiché — la recherche est une sous-chaîne simple).
  final String keywords;
  final Widget child;

  /// Identifiant `settingsDirtyProvider` quand la section supporte la
  /// sauvegarde différée (null = enregistrement immédiat uniquement).
  final String? sectionId;
  final key = GlobalKey();
}

final _kSections = <_SectionEntry>[
  _SectionEntry(
    id: _SectionId.appearance,
    keywords:
        'thème mode clair sombre accent couleur theme light dark '
        'color language langue',
    child: const AppearanceSection(),
  ),
  _SectionEntry(
    id: _SectionId.downloads,
    keywords:
        'destination dossier espace disque download_defaults saveas '
        'folder disk space default',
    sectionId: 'downloads',
    child: const DownloadsSection(),
  ),
  _SectionEntry(
    id: _SectionId.storage,
    keywords:
        'stockage storage zones public private privé chiffré '
        'move_on_completion default_area private_enabled espace '
        'portable orphelins orphans obd',
    child: const StorageSection(),
  ),
  _SectionEntry(
    id: _SectionId.bandwidth,
    keywords:
        'limite débit vitesse ko/s max_download_rate max_upload_rate '
        'limit rate speed kb/s',
    sectionId: 'bandwidth',
    child: const BandwidthSection(),
  ),
  _SectionEntry(
    id: _SectionId.queue,
    keywords:
        'queue active_downloads active_seeds active_checking '
        'active_limit auto_managed fastresume vérification démarrage '
        'startup check',
    sectionId: 'queue',
    child: const QueueSection(),
  ),
  _SectionEntry(
    id: _SectionId.seeding,
    keywords:
        'seeding ratio durée hops sauts safe seeding '
        'download_defaults number_anon_downloads duration default',
    sectionId: 'seeding',
    child: const SeedingSection(),
  ),
  _SectionEntry(
    id: _SectionId.anonymity,
    keywords:
        'tunnel community circuits min_circuits max_circuits '
        'exitnode sortie test vitesse exit speed anonymous',
    sectionId: 'anonymity',
    child: const AnonymitySection(),
  ),
  _SectionEntry(
    id: _SectionId.onionbit,
    keywords:
        'onionbit ext extension ledger registre comptabilité '
        'accounting obf obfuscation curateurs curators trust '
        'confiance attest sign-then-serve enforce msg_v1',
    sectionId: 'onionbit',
    child: const OnionBitSection(),
  ),
  _SectionEntry(
    id: _SectionId.stealth,
    keywords:
        'stealth furtif censure censure-resistant pont bridge '
        'onionbit-bridge invitation cover traffic camouflage '
        'role client gateway passerelle',
    sectionId: 'stealth',
    child: const StealthSection(),
  ),
  _SectionEntry(
    id: _SectionId.identity,
    keywords:
        'identité identity clé key export import backup sauvegarde '
        'restaurer restore obid ipv8_keypair nomade portable',
    sectionId: 'identity',
    child: const IdentitySection(),
  ),
  _SectionEntry(
    id: _SectionId.network,
    keywords:
        'dht upnp natpmp lsd utp proxy socks port écoute '
        'listen_interface listen',
    sectionId: 'network',
    child: const NetworkSection(),
  ),
  _SectionEntry(
    id: _SectionId.automation,
    keywords: 'watch folder rss flux dossier surveillance items feed',
    sectionId: 'automation',
    child: const AutomationSection(),
  ),
  _SectionEntry(
    id: _SectionId.versioning,
    keywords: 'version mise à jour update checker versioning upgrade',
    child: const VersioningSection(),
  ),
  _SectionEntry(
    id: _SectionId.connection,
    keywords: 'daemon clé api port http connexion key url',
    child: const ConnectionSection(),
  ),
  _SectionEntry(
    id: _SectionId.daemon,
    keywords: 'arrêt shutdown redémarrage logs stop restart state',
    child: const DaemonSection(),
  ),
];

/// Page « Réglages » — rail d'ancres + filtre + sections du catalogue.
class SettingsPage extends ConsumerStatefulWidget {
  const SettingsPage({super.key});

  @override
  ConsumerState<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends ConsumerState<SettingsPage> {
  String _filter = '';
  bool _savingAll = false;

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
      '${e.id.title(l10n)} ${e.keywords}'.toLowerCase().contains(
        _filter.toLowerCase(),
      );

  @override
  Widget build(BuildContext context) {
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
                horizontal: AppSpacing.md,
                vertical: AppSpacing.xs,
              ),
              child: Row(
                children: [
                  Icon(
                    Icons.edit_note,
                    size: 20,
                    color: scheme.onTertiaryContainer,
                  ),
                  const SizedBox(width: AppSpacing.sm),
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
                  const SizedBox(width: AppSpacing.xs),
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
            horizontal: AppSpacing.md,
            vertical: AppSpacing.xs,
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
                          padding: const EdgeInsets.only(right: AppSpacing.xs),
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
              const SizedBox(width: AppSpacing.sm),
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
                  padding: const EdgeInsets.all(AppSpacing.lg),
                  child: Center(child: Text(l10n.noMatchingSection(_filter))),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
