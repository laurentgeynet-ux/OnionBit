// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../app_design_theme.dart';
import '../primitives/frosted_surface.dart';
import '../tokens/color_tokens.dart';
import '../tokens/elevation_tokens.dart';
import '../tokens/motion_tokens.dart';
import '../tokens/radius_tokens.dart';
import '../tokens/spacing_tokens.dart';
import '../tokens/typography_tokens.dart';

/// Guide de style vivant du design system (ADR-0021 §7) — rend chaque
/// token/composant, sert de documentation ET de cible aux tests golden
/// (`test/design/`). Route `/_style-guide`, enregistrée uniquement en
/// `kDebugMode` (`core/router/app_router.dart`).
///
/// Outil de développement interne : pas d'i18n (hors périmètre ADR-0009
/// — au même titre que les journaux développeur), libellés en français.
/// Indépendant du thème ambiant de l'app : bascule clair/sombre locale
/// pour comparer les deux sans toucher aux préférences utilisateur.
class StyleGuidePage extends StatefulWidget {
  const StyleGuidePage({super.key, this.initialBrightness = Brightness.light});

  /// Exposé pour les tests golden (évite de simuler un tap sur le
  /// bouton de bascule juste pour atteindre le thème sombre).
  final Brightness initialBrightness;

  @override
  State<StyleGuidePage> createState() => _StyleGuidePageState();
}

class _StyleGuidePageState extends State<StyleGuidePage> {
  late Brightness _brightness = widget.initialBrightness;

  @override
  Widget build(BuildContext context) {
    final theme = _brightness == Brightness.light
        ? AppDesignTheme.light()
        : AppDesignTheme.dark();
    return Theme(
      data: theme,
      child: Builder(
        builder: (context) => Scaffold(
          appBar: AppBar(
            title: const Text('Guide de style — OnionBit (ADR-0021)'),
            actions: [
              IconButton(
                tooltip: 'Basculer clair/sombre',
                icon: Icon(
                  _brightness == Brightness.light
                      ? Icons.dark_mode_outlined
                      : Icons.light_mode_outlined,
                ),
                onPressed: () => setState(() {
                  _brightness = _brightness == Brightness.light
                      ? Brightness.dark
                      : Brightness.light;
                }),
              ),
            ],
          ),
          body: ListView(
            padding: const EdgeInsets.all(AppSpace.lg),
            children: const [
              _SectionTitle('Couleurs de marque'),
              _BrandColorsSwatches(),
              _SectionTitle('Couleurs sémantiques'),
              _SemanticColorsSwatches(),
              _SectionTitle('Paliers de surface (Material 3 natif)'),
              _SurfaceContainerSwatches(),
              _SectionTitle('Typographie'),
              _TypographySpecimen(),
              _SectionTitle('Monospace — hex / clés / seed phrases'),
              _MonoSpecimen(),
              _SectionTitle('Espacement'),
              _SpacingRuler(),
              _SectionTitle('Rayons'),
              _RadiusSamples(),
              _SectionTitle('Élévation'),
              _ElevationSamples(),
              _SectionTitle('Motion'),
              _MotionSamples(),
              _SectionTitle('Surface givrée (FrostedSurface)'),
              _FrostedSample(),
              SizedBox(height: AppSpace.xxl),
            ],
          ),
        ),
      ),
    );
  }
}

class _SectionTitle extends StatelessWidget {
  const _SectionTitle(this.label);
  final String label;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: AppSpace.xl, bottom: AppSpace.sm),
    child: Text(label, style: Theme.of(context).textTheme.headlineSmall),
  );
}

class _Swatch extends StatelessWidget {
  const _Swatch({required this.label, required this.color, required this.on});
  final String label;
  final Color color;
  final Color on;

  @override
  Widget build(BuildContext context) => Container(
    width: 140,
    padding: const EdgeInsets.all(AppSpace.sm),
    decoration: BoxDecoration(
      color: color,
      borderRadius: BorderRadius.circular(AppRadius.small),
    ),
    child: Text(
      label,
      style: Theme.of(
        context,
      ).textTheme.labelMedium?.copyWith(color: on),
    ),
  );
}

class _BrandColorsSwatches extends StatelessWidget {
  const _BrandColorsSwatches();

  @override
  Widget build(BuildContext context) => Wrap(
    spacing: AppSpace.sm,
    runSpacing: AppSpace.sm,
    children: const [
      _Swatch(
        label: 'seed #6C2EA6',
        color: AppBrandColors.seed,
        on: Colors.white,
      ),
      _Swatch(
        label: 'tertiary #4FD8E0',
        color: AppBrandColors.tertiary,
        on: Colors.black,
      ),
    ],
  );
}

class _SemanticColorsSwatches extends StatelessWidget {
  const _SemanticColorsSwatches();

  @override
  Widget build(BuildContext context) {
    final s = context.semanticColors;
    return Wrap(
      spacing: AppSpace.sm,
      runSpacing: AppSpace.sm,
      children: [
        _Swatch(label: 'success', color: s.success, on: s.onSuccess),
        _Swatch(
          label: 'successContainer',
          color: s.successContainer,
          on: s.onSuccessContainer,
        ),
        _Swatch(label: 'warning', color: s.warning, on: s.onWarning),
        _Swatch(
          label: 'warningContainer',
          color: s.warningContainer,
          on: s.onWarningContainer,
        ),
        _Swatch(label: 'info', color: s.info, on: s.onInfo),
        _Swatch(
          label: 'infoContainer',
          color: s.infoContainer,
          on: s.onInfoContainer,
        ),
        _Swatch(
          label: 'error (M3)',
          color: Theme.of(context).colorScheme.error,
          on: Theme.of(context).colorScheme.onError,
        ),
      ],
    );
  }
}

class _SurfaceContainerSwatches extends StatelessWidget {
  const _SurfaceContainerSwatches();

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final on = scheme.onSurface;
    return Wrap(
      spacing: AppSpace.sm,
      runSpacing: AppSpace.sm,
      children: [
        _Swatch(label: 'surfaceContainerLowest', color: scheme.surfaceContainerLowest, on: on),
        _Swatch(label: 'surfaceContainerLow', color: scheme.surfaceContainerLow, on: on),
        _Swatch(label: 'surfaceContainer', color: scheme.surfaceContainer, on: on),
        _Swatch(label: 'surfaceContainerHigh', color: scheme.surfaceContainerHigh, on: on),
        _Swatch(label: 'surfaceContainerHighest', color: scheme.surfaceContainerHighest, on: on),
      ],
    );
  }
}

class _TypographySpecimen extends StatelessWidget {
  const _TypographySpecimen();

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context).textTheme;
    final rows = <(String, TextStyle?)>[
      ('displayLarge — Space Grotesk', t.displayLarge),
      ('headlineMedium — Space Grotesk', t.headlineMedium),
      ('titleLarge — Space Grotesk', t.titleLarge),
      ('titleMedium — Inter', t.titleMedium),
      ('bodyLarge — Inter', t.bodyLarge),
      ('bodyMedium — Inter', t.bodyMedium),
      ('labelLarge — Inter', t.labelLarge),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final (label, style) in rows)
          Padding(
            padding: const EdgeInsets.only(bottom: AppSpace.xs),
            child: Text('OnionBit — toile onion sans serveur ($label)', style: style),
          ),
      ],
    );
  }
}

class _MonoSpecimen extends StatelessWidget {
  const _MonoSpecimen();

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      // Chaîne volontairement ambiguë en police standard — démontre
      // la distinction 0/O et 1/l de JetBrains Mono (ADR-0021 §3).
      Text('O0O0 Il1l1 — ambigu en police de corps', style: Theme.of(context).textTheme.bodyMedium),
      const SizedBox(height: AppSpace.xs),
      Text('O0O0 Il1l1 — sans ambiguïté en mono', style: AppTypography.mono(fontSize: 16)),
      const SizedBox(height: AppSpace.sm),
      Text(
        'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 (infohash)',
        style: AppTypography.mono(),
      ),
    ],
  );
}

class _SpacingRuler extends StatelessWidget {
  const _SpacingRuler();

  @override
  Widget build(BuildContext context) {
    const values = <(String, double)>[
      ('xs', AppSpace.xs),
      ('sm', AppSpace.sm),
      ('md', AppSpace.md),
      ('lg', AppSpace.lg),
      ('xl', AppSpace.xl),
      ('xxl', AppSpace.xxl),
    ];
    final color = Theme.of(context).colorScheme.primary;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final (name, value) in values)
          Padding(
            padding: const EdgeInsets.only(bottom: AppSpace.xs),
            child: Row(
              children: [
                SizedBox(width: 40, child: Text(name)),
                Container(width: value, height: 12, color: color),
                const SizedBox(width: AppSpace.xs),
                Text('${value.toInt()} dp'),
              ],
            ),
          ),
      ],
    );
  }
}

class _RadiusSamples extends StatelessWidget {
  const _RadiusSamples();

  @override
  Widget build(BuildContext context) {
    const values = <(String, double)>[
      ('small', AppRadius.small),
      ('medium', AppRadius.medium),
      ('large', AppRadius.large),
      ('full', 20),
    ];
    final color = Theme.of(context).colorScheme.secondaryContainer;
    return Wrap(
      spacing: AppSpace.sm,
      runSpacing: AppSpace.sm,
      children: [
        for (final (name, value) in values)
          Container(
            width: 72,
            height: 72,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: color,
              borderRadius: BorderRadius.circular(value),
            ),
            child: Text(name, style: Theme.of(context).textTheme.labelSmall),
          ),
      ],
    );
  }
}

class _ElevationSamples extends StatelessWidget {
  const _ElevationSamples();

  @override
  Widget build(BuildContext context) {
    const values = <(String, double)>[
      ('level1', AppElevation.level1),
      ('level2', AppElevation.level2),
      ('level3', AppElevation.level3),
      ('level4', AppElevation.level4),
      ('level5', AppElevation.level5),
    ];
    return Wrap(
      spacing: AppSpace.md,
      runSpacing: AppSpace.md,
      children: [
        for (final (name, value) in values)
          Material(
            elevation: value,
            borderRadius: BorderRadius.circular(AppRadius.medium),
            child: Container(
              width: 96,
              height: 64,
              alignment: Alignment.center,
              child: Text(name),
            ),
          ),
      ],
    );
  }
}

class _MotionSamples extends StatefulWidget {
  const _MotionSamples();

  @override
  State<_MotionSamples> createState() => _MotionSamplesState();
}

class _MotionSamplesState extends State<_MotionSamples> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(
        'fast=${AppMotionDuration.fast.inMilliseconds}ms · '
        'medium=${AppMotionDuration.medium.inMilliseconds}ms · '
        'slow=${AppMotionDuration.slow.inMilliseconds}ms',
      ),
      const SizedBox(height: AppSpace.sm),
      AnimatedContainer(
        duration: AppMotionDuration.slow,
        curve: AppMotionCurve.standard,
        width: _expanded ? 240 : 96,
        height: 48,
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.primaryContainer,
          borderRadius: BorderRadius.circular(AppRadius.medium),
        ),
      ),
      TextButton(
        onPressed: () => setState(() => _expanded = !_expanded),
        child: const Text('Déclencher (AppMotionCurve.standard)'),
      ),
    ],
  );
}

class _FrostedSample extends StatelessWidget {
  const _FrostedSample();

  @override
  Widget build(BuildContext context) => Stack(
    children: [
      Container(
        height: 140,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(AppRadius.large),
          gradient: LinearGradient(
            colors: [
              Theme.of(context).colorScheme.primary,
              Theme.of(context).colorScheme.tertiary,
            ],
          ),
        ),
      ),
      Positioned.fill(
        child: Padding(
          padding: const EdgeInsets.all(AppSpace.md),
          child: Align(
            alignment: Alignment.bottomLeft,
            child: FrostedSurface(
              child: Text(
                'Exemple : futur panneau Privacy HUD (ADR-0021 §8)',
                style: Theme.of(context).textTheme.bodyMedium,
              ),
            ),
          ),
        ),
      ),
    ],
  );
}
