// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/diagnostic/domain/diagnostic_models.dart';
import '../../features/diagnostic/presentation/providers/diagnostic_providers.dart';
import '../../features/privacy/presentation/widgets/privacy_profile_switch.dart';
import '../design/design_tokens.dart';
import '../l10n/l10n_ext.dart';
import 'breakpoints.dart';

/// Privacy HUD (ADR-0021 §8) — indicateur ambiant de la posture
/// d'anonymat dans la barre du haut : bouclier coloré + profondeur
/// réelle de la lane (circuits READY, `actualHops` du pire cas).
/// Sondé via `anonLaneProvider` (10 s — partagé avec la barre d'état,
/// aucun poll supplémentaire). Un tap ouvre l'écran Diagnostic ;
/// au palier `compact` (pas de sidebar), la feuille expose le
/// sélecteur de profil ADR-0022 + l'entrée Diagnostic.
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
          onTap: compact
              ? () => _ProfileSheet.show(context)
              : () => context.go('/diagnostic'),
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

/// Feuille « profil d'anonymat » du palier compact (ADR-0022 §5 —
/// la sidebar est absente à ce palier, le HUD porte l'accès) :
/// sélecteur trois positions + entrée Diagnostic.
class _ProfileSheet extends StatelessWidget {
  const _ProfileSheet();

  static Future<void> show(BuildContext context) =>
      showModalBottomSheet(
        context: context,
        showDragHandle: true,
        builder: (_) => const _ProfileSheet(),
      );

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          AppSpace.md,
          0,
          AppSpace.md,
          AppSpace.md,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              l10n.privacyProfileTitle,
              style: Theme.of(context).textTheme.titleSmall,
            ),
            const SizedBox(height: AppSpace.sm),
            const PrivacyProfileSwitch(),
            const Divider(height: AppSpace.lg),
            ListTile(
              dense: true,
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.monitor_heart_outlined, size: 20),
              title: Text(l10n.privacyProfileDiagnose),
              onTap: () {
                Navigator.of(context).pop();
                context.go('/diagnostic');
              },
            ),
          ],
        ),
      ),
    );
  }
}
