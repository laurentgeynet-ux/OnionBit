// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:ui';

import 'package:flutter/material.dart';

import '../tokens/radius_tokens.dart';

/// Surface « givrée » (ADR-0021 §7) : flou + dégradé léger plutôt que
/// les overlays d'élévation plats de Material par défaut — réservée
/// aux moments de marque (panneaux contextuels, Privacy HUD §8), pas
/// à chaque `Card` de l'application. Premier primitif du design
/// system ; les autres (boutons, champs, chips, navigation, dialogues
/// — ADR-0021 §3) arrivent au fil des écrans migrés (étapes 73+),
/// pilotés par leur usage réel plutôt que devinés à l'avance.
class FrostedSurface extends StatelessWidget {
  const FrostedSurface({
    super.key,
    required this.child,
    this.borderRadius = const BorderRadius.all(
      Radius.circular(AppRadius.large),
    ),
    this.blurSigma = 18,
    this.padding = const EdgeInsets.all(16),
  });

  final Widget child;
  final BorderRadius borderRadius;
  final double blurSigma;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ClipRRect(
      borderRadius: borderRadius,
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: blurSigma, sigmaY: blurSigma),
        child: Container(
          padding: padding,
          decoration: BoxDecoration(
            borderRadius: borderRadius,
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                scheme.surfaceContainerHigh.withValues(alpha: 0.72),
                scheme.surfaceContainerHighest.withValues(alpha: 0.55),
              ],
            ),
            border: Border.all(
              color: scheme.outlineVariant.withValues(alpha: 0.4),
            ),
          ),
          child: child,
        ),
      ),
    );
  }
}
