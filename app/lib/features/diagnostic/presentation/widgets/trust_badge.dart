// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../providers/diagnostic_providers.dart';

/// Pastille de confiance locale (ADR-0015 §6) : `+n` vert si les
/// curateurs suivis endorsent le sujet, `-n` rouge s'il est flagué,
/// gris si attestations sans verdict suivi. **N'affiche rien** quand
/// le sujet est inconnu — les lignes restent propres.
///
/// `extTrustProvider` est à requête unique (pas de tick) : le score
/// n'évolue que par attestation, un sondage par ligne serait une
/// amplification N+1.
///
/// `kind` : `infohash` | `channel` | `identity` ; `subject` : hex.
class TrustBadge extends ConsumerWidget {
  const TrustBadge({super.key, required this.kind, required this.subject});

  final String kind;
  final String subject;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (subject.isEmpty) return const SizedBox.shrink();
    final trust = ref
        .watch(extTrustProvider((kind: kind, subject: subject)))
        .value;
    if (trust == null || trust.attestationCount == 0) {
      return const SizedBox.shrink();
    }
    final scheme = Theme.of(context).colorScheme;
    final (color, icon) = switch (trust.score) {
      > 0 => (scheme.primary, Icons.verified),
      < 0 => (scheme.error, Icons.gpp_bad),
      _ => (scheme.outline, Icons.shield_outlined),
    };
    final detail = trust.score != 0
        ? context.l10n.trustTooltipScored(
            trust.endorsements.length,
            trust.flags.length,
            trust.attestationCount,
          )
        : context.l10n.trustTooltipUnscored(trust.attestationCount);
    return Padding(
      padding: const EdgeInsets.only(left: AppSpacing.xs),
      child: Tooltip(
        message: detail,
        child: Icon(icon, size: 14, color: color),
      ),
    );
  }
}
