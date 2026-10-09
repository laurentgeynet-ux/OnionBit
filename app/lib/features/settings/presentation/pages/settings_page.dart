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
  // Pas de GlobalKey ici : la liste est partagée entre instances —
  // une clé embarquée garderait un contexte périmé d'une page à
  // l'autre (et collisionnerait si deux pages coexistaient). Les clés
  // d'ancre vivent dans `_SettingsPageState._sectionKeys`.
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
  final _scrollController = ScrollController();

  /// Clés d'ancre par section — par instance de page (`_kSections` est
  /// global : y stocker des `GlobalKey` laisserait des contextes
  /// périmés entre pages/tests).
  final _sectionKeys = {
    for (final e in _kSections) e.id: GlobalKey(),
  };

  /// Catégories repliables (ADR-0021 §5, étape 75) — toutes ouvertes
  /// par défaut : la découverte prime ; le repli sert à réduire le
  /// bruit visuel une fois la section connue.
  final _expanded = {for (final c in SettingsCategory.values) c: true};
  final _catKeys = {
    for (final c in SettingsCategory.values) c: GlobalKey(),
  };

  /// Défile jusqu'à la section `?s=` demandée — post-frame (les
  /// `GlobalKey` des entrées n'existent qu'après le premier layout) et
  /// une fois par valeur (le paramètre peut changer sans recréer la
  /// page, ex. deux commandes réglages d'affilée).
  void _scrollToRequested() {
    final wanted = widget.sectionId;
    if (wanted == null || wanted == _scrolledTo) return;
    _scrolledTo = wanted;
    _aligned = false;
    _lastPixels = -1;
    _lastExtent = -1;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _SectionEntry? target;
      for (final e in _kSections) {
        if (e.id.name == wanted) target = e;
      }
      if (target == null) return;
      final cat = target.id.category;
      // Filtre actif ou catégorie repliée masqueraient la cible : le
      // deep-link est une intention de navigation — ré-affiche tout
      // puis défile à la frame suivante (l'ancre n'existe qu'une fois
      // rendue).
      if (_filter.isNotEmpty || _expanded[cat] == false) {
        setState(() {
          _filter = '';
          _expanded[cat] = true;
        });
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) _doScroll(wanted);
        });
        return;
      }
      _doScroll(wanted);
    });
  }

  @override
  void dispose() {
    _scrollController.dispose();
    super.dispose();
  }

  /// Dernier couple (pixels, étendue) observé par `_doScroll` — sert à
  /// détecter la stabilité du contenu ; `_aligned` marque que
  /// `ensureVisible` a déjà été lancé pour le montage courant (le
  /// relancer à chaque frame réinitialiserait l'animation sans fin).
  double _lastPixels = -1;
  double _lastExtent = -1;
  bool _aligned = false;

  /// Défile jusqu'à la section `wanted` (`SettingsSectionId.name`).
  /// La `ListView` est paresseuse : une section hors viewport n'a pas
  /// encore de contexte — on saute par écrans dans la bonne direction
  /// (comparaison de l'indice cible au premier élément monté) jusqu'à
  /// ce qu'elle apparaisse, puis `ensureVisible` affine.
  ///
  /// Piège : à la première frame les providers sont encore en
  /// chargement, les sections rendent des états compacts et la cible
  /// peut être montée prématurément — puis le contenu grandit à la
  /// résolution des données et la pousse hors du `cacheExtent` (elle
  /// se démonte, sa clé perd son contexte). On ne s'arrête donc que
  /// lorsque position **et** étendue sont identiques deux frames de
  /// suite, en reprenant les sauts si la cible a disparu entre-temps.
  void _doScroll(String wanted, [int attempt = 0]) {
    if (attempt > 40) return; // borne — une frame par essai.
    var targetIndex = -1;
    _SectionEntry? entry;
    for (var i = 0; i < _kSections.length; i++) {
      if (_kSections[i].id.name == wanted) {
        entry = _kSections[i];
        targetIndex = i;
      }
    }
    if (entry == null) return;
    final pos = _scrollController.hasClients
        ? _scrollController.position
        : null;
    final ctx = _sectionKeys[entry.id]?.currentContext;
    if (ctx != null) {
      if (!_aligned) {
        _aligned = true;
        Scrollable.ensureVisible(
          ctx,
          duration: const Duration(milliseconds: 250),
        );
      }
      if (pos != null &&
          pos.hasPixels &&
          pos.pixels == _lastPixels &&
          pos.maxScrollExtent == _lastExtent) {
        return; // contenu stabilisé, cible montée : terminé.
      }
      _lastPixels = pos != null && pos.hasPixels ? pos.pixels : -1;
      _lastExtent = pos != null && pos.hasPixels
          ? pos.maxScrollExtent
          : -1;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _doScroll(wanted, attempt + 1);
      });
      return;
    }
    // La cible n'est plus montée (re-sortie du cacheExtent après une
    // croissance du contenu) — il faudra ré-aligner au prochain
    // montage.
    _aligned = false;
    _lastPixels = -1;
    _lastExtent = -1;
    if (pos == null || !pos.hasViewportDimension) {
      // Le viewport n'est pas encore mesuré — réessayer à la frame
      // suivante plutôt que d'abandonner.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _doScroll(wanted, attempt + 1);
      });
      return;
    }
    // Premier indice monté = haut du viewport : si la cible est avant,
    // on remonte d'un écran, sinon on descend.
    var firstBuilt = _kSections.length;
    for (var i = 0; i < _kSections.length; i++) {
      if (_sectionKeys[_kSections[i].id]?.currentContext != null) {
        firstBuilt = i;
        break;
      }
    }
    final step = pos.viewportDimension;
    final next = targetIndex >= firstBuilt
        ? pos.pixels + step
        : pos.pixels - step;
    _scrollController.jumpTo(next.clamp(0.0, pos.maxScrollExtent));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _doScroll(wanted, attempt + 1);
    });
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
                // Rail d'ancres — une chip par catégorie (étape 75 ;
                // les sections restent joignables via la palette
                // Ctrl/Cmd+K et le deep-link `?s=`).
                child: SizedBox(
                  height: 34,
                  child: ListView(
                    scrollDirection: Axis.horizontal,
                    children: [
                      for (final cat in SettingsCategory.values)
                        if (visible.any((e) => e.id.category == cat))
                          Padding(
                            padding: const EdgeInsets.only(
                              right: AppSpace.xs,
                            ),
                            child: ActionChip(
                              avatar:
                                  visible.any(
                                    (e) =>
                                        e.id.category == cat &&
                                        e.sectionId != null &&
                                        dirty.contains(e.sectionId),
                                  )
                                  ? Icon(
                                      Icons.circle,
                                      size: 8,
                                      color: scheme.tertiary,
                                    )
                                  : null,
                              label: Text(cat.title(l10n)),
                              visualDensity: VisualDensity.compact,
                              onPressed: () {
                                setState(() => _expanded[cat] = true);
                                final ctx = _catKeys[cat]?.currentContext;
                                if (ctx != null) {
                                  Scrollable.ensureVisible(
                                    ctx,
                                    duration: const Duration(
                                      milliseconds: 250,
                                    ),
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
            controller: _scrollController,
            children: [
              for (final cat in SettingsCategory.values) ...[
                if (visible.any((e) => e.id.category == cat))
                  KeyedSubtree(
                    key: _catKeys[cat],
                    child: _CategoryHeader(
                      category: cat,
                      expanded: _expanded[cat]!,
                      onToggle: () => setState(
                        () => _expanded[cat] = !_expanded[cat]!,
                      ),
                    ),
                  ),
                // Pendant un filtre, les sections correspondantes
                // restent visibles même sous un en-tête replié.
                if (_expanded[cat]! || _filter.isNotEmpty)
                  for (final e in visible.where(
                    (e) => e.id.category == cat,
                  ))
                    KeyedSubtree(
                      key: _sectionKeys[e.id],
                      child: e.child,
                    ),
              ],
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

/// En-tête de catégorie repliable (étape 75 — « réglages par
/// paliers ») : icône + titre en petites capitales + chevron ; un tap
/// replie/déplie le groupe de sections en dessous.
class _CategoryHeader extends StatelessWidget {
  const _CategoryHeader({
    required this.category,
    required this.expanded,
    required this.onToggle,
  });

  final SettingsCategory category;
  final bool expanded;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return InkWell(
      onTap: onToggle,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          AppSpace.md,
          AppSpace.md,
          AppSpace.sm,
          AppSpace.xs,
        ),
        child: Row(
          children: [
            Icon(category.icon, size: 18, color: theme.colorScheme.primary),
            const SizedBox(width: AppSpace.sm),
            Expanded(
              child: Text(
                category.title(context.l10n).toUpperCase(),
                style: theme.textTheme.labelLarge?.copyWith(
                  letterSpacing: 1.1,
                  color: theme.colorScheme.primary,
                ),
              ),
            ),
            Icon(
              expanded ? Icons.expand_less : Icons.expand_more,
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ],
        ),
      ),
    );
  }
}
