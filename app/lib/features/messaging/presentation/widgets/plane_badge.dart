// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';

/// Pastille de plan de transport (ADR-0029 §8 Q3) — chaque
/// message, contact et conversation indique le plan réellement
/// emprunté ou disponible, sans mélange :
///
/// - vert `OnionBit` : circuits anonymes IPv8 (plan propre) ;
/// - violet `Tor` : service `.onion` (plan Tor — jamais de trafic
///   OnionBit dedans) ;
/// - ambre `boîte aux lettres` : dépôt store-and-forward
///   (remise différée, ADR-0026).
///
/// Le libellé complet vit dans le tooltip : la pastille reste
/// discrète dans les bulles et les tuiles.
class PlaneDot extends StatelessWidget {
  const PlaneDot({
    super.key,
    required this.plane,
    this.size = 9,
    this.tooltip,
  });

  /// `"onionbit"`/`"ipv8"`, `"onion"`, `"mailbox"` (valeurs du DTO).
  final String plane;

  /// Diamètre de la pastille.
  final double size;

  /// Tooltip de substitution (ex. état de vérification d'une
  /// liaison `.onion`) — `null` = libellé standard du plan.
  final String? tooltip;

  /// Couleur du plan — `null` pour une valeur inconnue (rien
  /// n'est rendu : un plan inconnu n'est pas affublé d'une
  /// couleur au hasard).
  static Color? colorOf(String plane) => switch (plane) {
    'onionbit' || 'ipv8' => AppPlaneColors.onionbit,
    'onion' => AppPlaneColors.tor,
    'mailbox' => AppPlaneColors.mailbox,
    _ => null,
  };

  static String _labelOf(BuildContext context, String plane) =>
      switch (plane) {
        'onionbit' || 'ipv8' => context.l10n.msgPlaneOnionbit,
        'onion' => context.l10n.msgPlaneTor,
        'mailbox' => context.l10n.msgPlaneMailbox,
        _ => plane,
      };

  @override
  Widget build(BuildContext context) {
    final color = colorOf(plane);
    if (color == null) return const SizedBox.shrink();
    final label = tooltip ?? _labelOf(context, plane);
    return Tooltip(
      message: label,
      child: Semantics(
        label: label,
        child: Container(
          width: size,
          height: size,
          decoration: BoxDecoration(
            color: color,
            shape: BoxShape.circle,
            border: Border.all(
              color: Theme.of(context).colorScheme.surface,
              width: 1,
            ),
          ),
        ),
      ),
    );
  }
}

/// Rangée de pastilles pour les `planes` d'un contact ou d'une
/// conversation (`["onionbit", "onion"]`). Plans inconnus ignorés.
class PlaneBadges extends StatelessWidget {
  const PlaneBadges({super.key, required this.planes, this.size = 9});

  /// Plans d'adressage — ordre conservé tel que servi par l'API.
  final List<String> planes;

  /// Diamètre de chaque pastille.
  final double size;

  @override
  Widget build(BuildContext context) {
    final dots = [
      for (final p in planes)
        if (PlaneDot.colorOf(p) != null)
          Padding(
            padding: const EdgeInsets.only(left: 3),
            child: PlaneDot(plane: p, size: size),
          ),
    ];
    if (dots.isEmpty) return const SizedBox.shrink();
    return Row(mainAxisSize: MainAxisSize.min, children: dots);
  }
}
