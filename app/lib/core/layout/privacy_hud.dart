// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/diagnostic/domain/diagnostic_models.dart';
import '../../features/diagnostic/presentation/providers/diagnostic_providers.dart';
import '../design/design_tokens.dart';
import '../l10n/l10n_ext.dart';
import 'breakpoints.dart';

/// Privacy HUD (ADR-0021 §8) — indicateur ambiant de la posture
/// d'anonymat dans la barre du haut : bouclier coloré + profondeur
/// réelle de la lane (circuits READY, `actualHops` du pire cas).
/// Sondé via `anonLaneProvider` (10 s — partagé avec la barre d'état,
/// aucun poll supplémentaire). Un tap ouvre l'écran Diagnostic.
///
/// États « honnêtes » hérités d'`AnonLaneStatus` : `disabled` = stack
/// IPv8 inactive (pas « en panne »), `waiting` = circuits en
/// construction, `ready` = au moins un circuit vérifié.
class PrivacyHud extends ConsumerWidget {
  const PrivacyHud({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final lane = ref.watch(anonLaneProvider).value;
    final scheme = Theme.of(context).colorScheme;
    final semantic = context.semanticColors;
    final compact =
        AppBreakpoints.of(MediaQuery.sizeOf(context).width) ==
        AppBreakpoint.compact;

    final (icon, color, label) = switch (lane?.state) {
      AnonLaneState.ready => (
        Icons.shield,
        semantic.success,
        context.l10n.hudAnonReady(lane!.readyCircuits, lane.minReadyHops),
      ),
      AnonLaneState.waiting => (
        Icons.shield_outlined,
        semantic.warning,
        context.l10n.hudAnonWaiting,
      ),
      _ => (
        Icons.shield_outlined,
        scheme.outline,
        context.l10n.hudAnonDisabled,
      ),
    };

    // `Semantics` explicite : en `compact` l'indicateur est une icône
    // seule — sans label, un lecteur d'écran n'annoncerait rien.
    return Semantics(
      button: true,
      label: '$label — ${context.l10n.hudAnonTooltip}',
      child: Tooltip(
        message: context.l10n.hudAnonTooltip,
        child: InkWell(
          borderRadius: BorderRadius.circular(AppRadius.full),
          onTap: () => context.go('/diagnostic'),
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: AppSpace.sm,
              vertical: AppSpace.xs,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(icon, size: 18, color: color),
                if (!compact) ...[
                  const SizedBox(width: AppSpace.xs),
                  Text(
                    label,
                    style: Theme.of(
                      context,
                    ).textTheme.labelMedium?.copyWith(color: color),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
