// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';
import '../widgets/advanced_section.dart';
import '../widgets/anonymity_section.dart';
import '../widgets/appearance_section.dart';
import '../widgets/automation_section.dart';
import '../widgets/bandwidth_section.dart';
import '../widgets/connection_section.dart';
import '../widgets/daemon_section.dart';
import '../widgets/downloads_section.dart';
import '../widgets/network_section.dart';
import '../widgets/queue_section.dart';
import '../widgets/seeding_section.dart';
import '../widgets/versioning_section.dart';

/// Entrée du catalogue des sections — titre de l'ancre + mots-clés de
/// recherche (chemins de clés inclus).
class _SectionEntry {
  _SectionEntry({
    required this.title,
    required this.keywords,
    required this.child,
    this.sectionId,
  });

  final String title;

  /// Texte de recherche : titre + noms de champs + chemins de clés.
  final String keywords;
  final Widget child;

  /// Identifiant `settingsDirtyProvider` quand la section supporte la
  /// sauvegarde différée (null = enregistrement immédiat uniquement).
  final String? sectionId;
  final key = GlobalKey();
}

final _kSections = <_SectionEntry>[
  _SectionEntry(
    title: 'Apparence',
    keywords: 'thème mode clair sombre accent couleur',
    child: const AppearanceSection(),
  ),
  _SectionEntry(
    title: 'Téléchargements par défaut',
    keywords: 'destination dossier espace disque download_defaults saveas',
    sectionId: 'downloads',
    child: const DownloadsSection(),
  ),
  _SectionEntry(
    title: 'Bande passante',
    keywords: 'limite débit vitesse ko/s max_download_rate max_upload_rate',
    sectionId: 'bandwidth',
    child: const BandwidthSection(),
  ),
  _SectionEntry(
    title: 'File d\'attente',
    keywords:
        'queue active_downloads active_seeds active_checking '
        'active_limit auto_managed fastresume vérification démarrage',
    sectionId: 'queue',
    child: const QueueSection(),
  ),
  _SectionEntry(
    title: 'Seed & anonymat par défaut',
    keywords:
        'seeding ratio durée hops sauts safe seeding '
        'download_defaults number_anon_downloads',
    sectionId: 'seeding',
    child: const SeedingSection(),
  ),
  _SectionEntry(
    title: 'Tunnels anonymes',
    keywords:
        'tunnel community circuits min_circuits max_circuits '
        'exitnode sortie test vitesse',
    sectionId: 'anonymity',
    child: const AnonymitySection(),
  ),
  _SectionEntry(
    title: 'Réseau',
    keywords:
        'dht upnp natpmp lsd utp proxy socks port écoute '
        'listen_interface',
    sectionId: 'network',
    child: const NetworkSection(),
  ),
  _SectionEntry(
    title: 'Automatisation',
    keywords: 'watch folder rss flux dossier surveillance items',
    sectionId: 'automation',
    child: const AutomationSection(),
  ),
  _SectionEntry(
    title: 'Mises à jour',
    keywords: 'version mise à jour update checker versioning',
    child: const VersioningSection(),
  ),
  _SectionEntry(
    title: 'Connexion',
    keywords: 'daemon clé api port http connexion key',
    child: const ConnectionSection(),
  ),
  _SectionEntry(
    title: 'Configuration avancée',
    sectionId: 'advanced',
    keywords: 'json arbre brut expert toutes les clés configuration',
    child: const AdvancedSection(),
  ),
  _SectionEntry(
    title: 'Daemon',
    keywords: 'arrêt shutdown redémarrage logs',
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

  bool _matches(_SectionEntry e) =>
      _filter.isEmpty ||
      '${e.title} ${e.keywords}'.toLowerCase().contains(_filter.toLowerCase());

  @override
  Widget build(BuildContext context) {
    final visible = _kSections.where(_matches).toList();
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
                      '${dirty.length} section(s) modifiée(s) '
                      'non enregistrée(s)',
                      style: Theme.of(context).textTheme.bodySmall
                          ?.copyWith(color: scheme.onTertiaryContainer),
                    ),
                  ),
                  TextButton(
                    onPressed: _discardAll,
                    child: const Text('Tout annuler'),
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
                    label: const Text('Enregistrer tout'),
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
                            label: Text(e.title),
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
                  decoration: const InputDecoration(
                    hintText: 'Filtrer les réglages',
                    isDense: true,
                    prefixIcon: Icon(Icons.filter_list, size: 18),
                    border: OutlineInputBorder(),
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
                  child: Center(
                    child: Text('Aucune section ne correspond à « $_filter ».'),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
