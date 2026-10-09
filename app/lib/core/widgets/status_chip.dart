// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../design/design_tokens.dart';

/// Puce de statut compacte (connecté/déconnecté, seeding/arrêté…),
/// réutilisée par toutes les features plutôt que réimplémentée par écran.
class StatusChip extends StatelessWidget {
  const StatusChip({super.key, required this.label, required this.tone});

  final String label;
  final StatusTone tone;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final semantic = context.semanticColors;
    final (background, foreground) = switch (tone) {
      StatusTone.positive => (
        semantic.successContainer,
        semantic.onSuccessContainer,
      ),
      StatusTone.neutral => (
        scheme.surfaceContainerHighest,
        scheme.onSurfaceVariant,
      ),
      StatusTone.warning => (
        semantic.warningContainer,
        semantic.onWarningContainer,
      ),
      StatusTone.negative => (scheme.errorContainer, scheme.onErrorContainer),
    };

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        color: background,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Text(
        label,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        textAlign: TextAlign.center,
        style: Theme.of(context).textTheme.labelMedium
            ?.copyWith(color: foreground),
      ),
    );
  }
}

enum StatusTone { positive, neutral, warning, negative }
