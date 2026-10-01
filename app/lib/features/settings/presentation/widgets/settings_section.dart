// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';

/// Carte « section de réglages » — titre + icône + contenu chargé des
/// réglages du daemon (`GET /api/settings`). Factorise le scaffolding
/// Card/AsyncValue répété par chaque section.
class SettingsSection extends ConsumerWidget {
  const SettingsSection({
    super.key,
    required this.icon,
    required this.title,
    required this.child,
    this.sectionId,
    this.defaults,
  });

  final IconData icon;
  final String title;

  /// Id de la section — affiche la puce « modifié » quand présent dans
  /// `settingsDirtyProvider` (sections à sauvegarde différée).
  final String? sectionId;

  /// Patch « rétablir les défauts » (`settings_defaults.dart`) —
  /// affiche le bouton reset dans l'en-tête quand fourni.
  final Map<String, dynamic>? defaults;

  /// Construit le contenu depuis l'arbre de réglages du daemon.
  final Widget Function(BuildContext context, Map<String, dynamic> settings)
  child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
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
                Icon(icon, size: 20),
                const SizedBox(width: AppSpacing.sm),
                Expanded(
                  child: Text(title, style: theme.textTheme.titleMedium),
                ),
                if (defaults != null)
                  IconButton(
                    tooltip: 'Rétablir les défauts de la section',
                    icon: const Icon(Icons.restart_alt, size: 18),
                    visualDensity: VisualDensity.compact,
                    onPressed: () async {
                      await applySettingsPatch(
                        context,
                        ref,
                        defaults!,
                        successMessage: 'Défauts restaurés',
                      );
                      // Ré-synchronise les champs locaux des sections à
                      // sauvegarde différée depuis la nouvelle config.
                      if (sectionId != null) {
                        ref.read(settingsSaveBusProvider)[sectionId]?.discard();
                      }
                    },
                  ),
                if (sectionId != null &&
                    ref.watch(
                      settingsDirtyProvider.select(
                        (s) => s.contains(sectionId),
                      ),
                    ))
                  Tooltip(
                    message: 'Modifications non enregistrées',
                    child: Icon(
                      Icons.circle,
                      size: 10,
                      color: theme.colorScheme.tertiary,
                    ),
                  ),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            async.when(
              loading: () => const Padding(
                padding: EdgeInsets.all(AppSpacing.sm),
                child: LinearProgressIndicator(),
              ),
              error: (e, _) => Text(
                '$e',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              ),
              data: (s) => child(context, s),
            ),
          ],
        ),
      ),
    );
  }
}

/// Applique un patch de réglages (`POST /api/settings`, merge
/// récursif) puis invalide le cache — snackbar d'erreur si échec.
Future<void> applySettingsPatch(
  BuildContext context,
  WidgetRef ref,
  Map<String, dynamic> patch, {
  String? successMessage,
}) async {
  try {
    await ref.read(settingsRepositoryProvider).update(patch);
    ref.invalidate(daemonSettingsProvider);
    if (successMessage != null && context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(successMessage)));
    }
  } catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Erreur : $e')));
    }
  }
}

/// Interrupteur de réglage à sauvegarde immédiate (sous-arbre
/// `{section: {key: value}}` — `path` = `['section', 'key', …]`).
class SettingsSwitch extends ConsumerWidget {
  const SettingsSwitch({
    super.key,
    required this.path,
    required this.value,
    required this.title,
    this.subtitle,
    this.onChangedOverride,
  });

  /// Chemin des clés dans l'arbre (`['libtorrent', 'dht']`).
  final List<String> path;

  /// Valeur courante lue dans les réglages.
  final bool value;
  final String title;
  final String? subtitle;

  /// Quand fourni, le commutateur n'enregistre pas immédiatement :
  /// la section appelante gère l'état local et sauvegarde en lot
  /// (bouton « Enregistrer »).
  final ValueChanged<bool>? onChangedOverride;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return SwitchListTile(
      value: value,
      onChanged: (v) {
        if (onChangedOverride != null) {
          onChangedOverride!(v);
          return;
        }
        var patch = v as dynamic;
        for (final k in path.reversed) {
          patch = {k: patch};
        }
        applySettingsPatch(context, ref, patch as Map<String, dynamic>);
      },
      title: Row(
        children: [
          Expanded(child: Text(title)),
          KeyInfoIcon(path),
        ],
      ),
      subtitle: subtitle != null ? Text(subtitle!) : null,
      contentPadding: EdgeInsets.zero,
      dense: true,
    );
  }
}

/// Icône `i` affichant le chemin de la clé dans `configuration.json`
/// (`libtorrent/max_download_rate`) + une description optionnelle —
/// repère de debug/documentaire sur les champs de réglage.
class KeyInfoIcon extends StatelessWidget {
  const KeyInfoIcon(this.path, {super.key, this.description});

  /// Chemin de la clé dans l'arbre (`['libtorrent', 'dht']`).
  final List<String> path;
  final String? description;

  @override
  Widget build(BuildContext context) {
    final shown = path.join('/');
    return Tooltip(
      message: description != null ? '$shown\n$description' : shown,
      preferBelow: false,
      child: Icon(
        Icons.info_outline,
        size: 14,
        color: Theme.of(context).colorScheme.outline,
      ),
    );
  }
}

/// Lit une feuille de l'arbre de réglages (`settings['a']['b']`).
Object? settingsLeaf(Map<String, dynamic> settings, List<String> path) {
  Object? node = settings;
  for (final k in path) {
    if (node is! Map<String, dynamic>) return null;
    node = node[k];
  }
  return node;
}

bool settingsBool(
  Map<String, dynamic> s,
  List<String> path, {
  bool def = false,
}) => settingsLeaf(s, path) == true || (settingsLeaf(s, path) == null && def);

int settingsInt(Map<String, dynamic> s, List<String> path, {int def = 0}) =>
    (settingsLeaf(s, path) as num?)?.toInt() ?? def;

double settingsDouble(
  Map<String, dynamic> s,
  List<String> path, {
  double def = 0,
}) => (settingsLeaf(s, path) as num?)?.toDouble() ?? def;

/// Lien entre une section à sauvegarde différée et le bus global :
/// enregistre `save`/`discard`, gère la puce « modifié ». Usage :
/// créer dans `initState` (`late final`), `attach`/`detach`,
/// `markDirty()` sur changement de champ, `markClean()` après succès.
class DeferredSection {
  DeferredSection(this._ref, this.id);

  final WidgetRef _ref;

  /// Identifiant stable de la section (`dirtyProvider`, bus).
  final String id;
  bool _dirty = false;

  /// Map partagée capturée à `attach` : `detach` est appelé depuis
  /// `dispose()` où `ref.read` est interdit (widget démonté) — la
  /// référence à la map du provider, elle, reste valide.
  Map<String, ({Future<void> Function() save, void Function() discard})>?
  _bus;

  void attach({
    required Future<void> Function() save,
    required void Function() discard,
  }) {
    _bus = _ref.read(settingsSaveBusProvider);
    _bus![id] = (save: save, discard: discard);
  }

  void detach() => _bus?.remove(id);

  void markDirty() {
    if (_dirty) return;
    _dirty = true;
    _ref.read(settingsDirtyProvider.notifier).add(id);
  }

  void markClean() {
    _dirty = false;
    _ref.read(settingsDirtyProvider.notifier).remove(id);
  }
}

String settingsString(
  Map<String, dynamic> s,
  List<String> path, {
  String def = '',
}) => '${settingsLeaf(s, path) ?? def}';
