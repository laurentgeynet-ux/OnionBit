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
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import 'breakpoints.dart';

/// Privacy HUD (ADR-0021 §8) — bandeau « état du réseau » dans la
/// barre du haut : une rangée de segments, un par plan — daemon
/// (SSE), overlay OnionBit (pairs découverts), tunnels anonymes
/// (circuits READY + pire cas de sauts) et lane Tor (état de
/// connexion uniquement — la santé des sources `.onion` reste au
/// chip de recherche). Chaque segment : icône colorée + tooltip
/// détaillé ; le tap ouvre l'écran Diagnostic (feuille profil
/// ADR-0022 au palier `compact`).
///
/// États « honnêtes » : désactivé ≠ panne — `off`/`disabled`
/// s'affichent en gris, jamais en rouge.
class PrivacyHud extends ConsumerWidget {
  const PrivacyHud({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final scheme = Theme.of(context).colorScheme;
    final semantic = context.semanticColors;
    final compact =
        AppBreakpoints.of(MediaQuery.sizeOf(context).width) ==
        AppBreakpoint.compact;

    // — Signaux (providers déjà sondés, aucun poll supplémentaire) —
    final connected = ref.watch(sseConnectedProvider).value ?? false;
    final lane = ref.watch(anonLaneProvider).value;
    final overlays = ref.watch(overlaysProvider).value;
    final tor = ref.watch(torStatusProvider).value;

    final peers = overlays?.fold<int>(0, (a, o) => a + o.peers) ?? 0;
    final communities = overlays?.length ?? 0;

    final segments = <(IconData, Color, String)>[
      // Daemon : flux SSE vivant = daemon joignable.
      (
        Icons.dns_outlined,
        connected ? semantic.success : scheme.error,
        connected ? l10n.netDaemonOk : l10n.netDaemonKo,
      ),
      // Overlay OnionBit : pairs découverts. Gris si la stack est
      // désactivée (lane `disabled` = IPv8/tunnel off).
      (
        Icons.hub_outlined,
        switch (lane?.state) {
          AnonLaneState.ready ||
          AnonLaneState.waiting => peers > 0
              ? semantic.success
              : semantic.warning,
          _ => scheme.outline,
        },
        switch (lane?.state) {
          AnonLaneState.ready ||
          AnonLaneState.waiting => peers > 0
              ? l10n.netOverlayOk(peers, communities)
              : l10n.netOverlayWaiting,
          _ => l10n.netOverlayOff,
        },
      ),
      // Tunnels anonymes : profondeur réelle (pire cas) — le
      // chiffre qui importe pour l'anonymat du téléchargement.
      (
        lane?.state == AnonLaneState.ready
            ? Icons.shield
            : Icons.shield_outlined,
        switch (lane?.state) {
          AnonLaneState.ready => semantic.success,
          AnonLaneState.waiting => semantic.warning,
          _ => scheme.outline,
        },
        switch (lane?.state) {
          AnonLaneState.ready => l10n.hudAnonReady(
            lane!.readyCircuits,
            lane.minReadyHops,
          ),
          AnonLaneState.waiting => l10n.hudAnonWaiting,
          _ => l10n.hudAnonDisabled,
        },
      ),
      // Lane Tor : état de connexion seul (ADR-0028) — vivante en
      // permanence quand `enabled` (messagerie/canaux/coffre
      // `.onion`, ADR-0029). Jamais de détail TPB ici.
      if (tor?.enabled ?? false)
        switch (tor!.state) {
          TorRuntimeState.ready => (
            Icons.vpn_lock,
            semantic.success,
            l10n.netTorReady,
          ),
          TorRuntimeState.bootstrapping => (
            Icons.vpn_lock_outlined,
            semantic.warning,
            l10n.torChipBootstrapping(tor.progress),
          ),
          TorRuntimeState.dormant => (
            Icons.vpn_lock_outlined,
            semantic.success,
            l10n.netTorDormant,
          ),
          TorRuntimeState.error => (
            Icons.vpn_lock_outlined,
            scheme.error,
            l10n.netTorError,
          ),
          _ => (Icons.vpn_lock_outlined, scheme.outline, l10n.netTorUnknown),
        }
      else
        (Icons.vpn_lock_outlined, scheme.outline, l10n.netTorOff),
    ];

    // `Semantics` explicite : icônes seules — sans label agrégé, un
    // lecteur d'écran n'annoncerait rien.
    return Semantics(
      button: true,
      label:
          '${segments.map((s) => s.$3).join(' · ')} — ${l10n.hudAnonTooltip}',
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
              for (final (icon, color, tip) in segments)
                Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpace.xs,
                  ),
                  child: Tooltip(
                    message: tip,
                    child: Icon(icon, size: 18, color: color),
                  ),
                ),
            ],
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
