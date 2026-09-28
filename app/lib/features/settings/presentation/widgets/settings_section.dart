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
  });

  final IconData icon;
  final String title;

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
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(successMessage)));
    }
  } catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Erreur : $e')));
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
      title: Text(title),
      subtitle: subtitle != null ? Text(subtitle!) : null,
      contentPadding: EdgeInsets.zero,
      dense: true,
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

bool settingsBool(Map<String, dynamic> s, List<String> path, {bool def = false}) =>
    settingsLeaf(s, path) == true || (settingsLeaf(s, path) == null && def);

int settingsInt(Map<String, dynamic> s, List<String> path, {int def = 0}) =>
    (settingsLeaf(s, path) as num?)?.toInt() ?? def;

double settingsDouble(
  Map<String, dynamic> s,
  List<String> path, {
  double def = 0,
}) =>
    (settingsLeaf(s, path) as num?)?.toDouble() ?? def;

String settingsString(
  Map<String, dynamic> s,
  List<String> path, {
  String def = '',
}) =>
    '${settingsLeaf(s, path) ?? def}';
