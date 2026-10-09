// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/l10n/locale_settings.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/theme/theme_settings.dart';

/// Section « Apparence » de la page Réglages — accent Material You +
/// mode clair/sombre/auto persistés via [themeSettingsProvider], et
/// sélecteur de langue via [localeSettingsProvider].
class AppearanceSection extends ConsumerWidget {
  const AppearanceSection({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final settings = ref.watch(themeSettingsProvider);
    final appLocale =
        ref.watch(localeSettingsProvider).value ?? AppLocale.en;

    return Card(
      margin: const EdgeInsets.all(AppSpace.md),
      child: Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: settings.when(
          data: (s) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                l10n.settingsAppearanceTitle,
                style: theme.textTheme.titleMedium,
              ),
              const SizedBox(height: AppSpace.sm),
              Text(
                l10n.settingsLanguage,
                style: theme.textTheme.labelMedium,
              ),
              const SizedBox(height: AppSpace.xs),
              SegmentedButton<AppLocale>(
                segments: [
                  ButtonSegment(
                    value: AppLocale.system,
                    icon: const Icon(Icons.language),
                    label: Text(l10n.languageSystem),
                  ),
                  const ButtonSegment(
                    value: AppLocale.en,
                    label: Text('English'),
                  ),
                  const ButtonSegment(
                    value: AppLocale.fr,
                    label: Text('Français'),
                  ),
                ],
                selected: {appLocale},
                onSelectionChanged: (m) => ref
                    .read(localeSettingsProvider.notifier)
                    .setLocale(m.first),
              ),
              const SizedBox(height: AppSpace.md),
              Text(
                l10n.settingsThemeMode,
                style: theme.textTheme.labelMedium,
              ),
              const SizedBox(height: AppSpace.xs),
              SegmentedButton<ThemeMode>(
                segments: [
                  ButtonSegment(
                    value: ThemeMode.system,
                    icon: const Icon(Icons.brightness_auto),
                    label: Text(l10n.themeModeAuto),
                  ),
                  ButtonSegment(
                    value: ThemeMode.light,
                    icon: const Icon(Icons.light_mode_outlined),
                    label: Text(l10n.themeModeLight),
                  ),
                  ButtonSegment(
                    value: ThemeMode.dark,
                    icon: const Icon(Icons.dark_mode_outlined),
                    label: Text(l10n.themeModeDark),
                  ),
                ],
                selected: {s.mode},
                onSelectionChanged: (m) =>
                    ref.read(themeSettingsProvider.notifier).setMode(m.first),
              ),
              const SizedBox(height: AppSpace.md),
              Text(l10n.settingsAccent, style: theme.textTheme.labelMedium),
              const SizedBox(height: AppSpace.xs),
              Wrap(
                spacing: AppSpace.sm,
                runSpacing: AppSpace.sm,
                children: [
                  for (final (accent, color) in kAccentChoices)
                    _AccentDot(
                      label: accent.label(l10n),
                      color: color,
                      selected: s.seedColor.toARGB32() == color.toARGB32(),
                      onTap: () => ref
                          .read(themeSettingsProvider.notifier)
                          .setSeedColor(color),
                    ),
                ],
              ),
            ],
          ),
          loading: () => const Center(child: CircularProgressIndicator()),
          error: (e, _) => Text('$e'),
        ),
      ),
    );
  }
}

class _AccentDot extends StatelessWidget {
  const _AccentDot({
    required this.label,
    required this.color,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final Color color;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: label,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(20),
        child: Container(
          width: 36,
          height: 36,
          decoration: BoxDecoration(
            color: color,
            shape: BoxShape.circle,
            border: selected
                ? Border.all(
                    color: Theme.of(context).colorScheme.onSurface,
                    width: 2.5,
                  )
                : null,
          ),
          child: selected
              ? const Icon(Icons.check, size: 18, color: Colors.white)
              : null,
        ),
      ),
    );
  }
}
