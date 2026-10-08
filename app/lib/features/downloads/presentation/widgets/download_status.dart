// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/widgets/status_chip.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import '../../domain/download.dart';

/// Barre de progression fusionnée avec le statut : le libellé localisé
/// (« Partage 100 % », « Arrêté »…) est centré sur la barre, dont la
/// couleur suit la tonalité du statut (mapping des statuts Python
/// `DLSTATUS_*`). Remplace l'ancien doublon « barre + puce » des
/// colonnes Progression/État — même modèle que la colonne Progression
/// de qBittorrent.
class DownloadProgressBar extends StatelessWidget {
  const DownloadProgressBar({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (label, tone) = _mapStatus(context.l10n, download);
    final color = switch (tone) {
      StatusTone.positive => scheme.primary,
      StatusTone.neutral => scheme.outline,
      StatusTone.warning => scheme.tertiary,
      StatusTone.negative => scheme.error,
    };
    return SizedBox(
      height: 16,
      child: Stack(
        alignment: Alignment.center,
        children: [
          ClipRRect(
            borderRadius: BorderRadius.circular(8),
            child: LinearProgressIndicator(
              value: download.progress.clamp(0.0, 1.0),
              minHeight: 16,
              color: color,
              backgroundColor: scheme.surfaceContainerHighest,
            ),
          ),
          // Pastille de surface translucide : le texte brut était
          // invisible sur la portion remplie en thème sombre, et la
          // barre fine le rendait flottant autour.
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 4),
            decoration: BoxDecoration(
              color: scheme.surface.withAlpha(200),
              borderRadius: BorderRadius.circular(4),
            ),
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.labelSmall,
            ),
          ),
        ],
      ),
    );
  }
}

/// Libellé + tonalité du statut d'un téléchargement — mapping des
/// statuts Python (`DLSTATUS_*`).
(String, StatusTone) _mapStatus(AppLocalizations l10n, Download d) {
  if (d.isError) return (l10n.statusError, StatusTone.negative);
  final pct = (d.progress * 100).toStringAsFixed(0);
  return switch (d.status) {
    'DOWNLOADING' => (
      l10n.statusDownloading(pct),
      StatusTone.positive,
    ),
    'SEEDING' => (l10n.statusSeeding(pct), StatusTone.positive),
    'METADATA' => (l10n.statusMetadata, StatusTone.warning),
    'HASHCHECKING' ||
    'WAITING_FOR_HASHCHECK' => (l10n.statusChecking, StatusTone.warning),
    'STOPPED' => (l10n.statusStopped, StatusTone.neutral),
    _ => (d.status.isEmpty ? '—' : d.status, StatusTone.neutral),
  };
}

/// Badge d'anonymat explicite : « Clair » (trafic direct, badge
/// d'avertissement rempli — IP exposée) ou « Anon ×N » (N sauts de
/// circuit, contour primaire). Couleur tertiaire = « en attente de
/// circuit » (le kill switch bloque le trafic sans circuit).
class AnonBadge extends StatelessWidget {
  const AnonBadge({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final scheme = Theme.of(context).colorScheme;
    final hops = download.hops;
    final anon = download.anonDownload && hops > 0;
    final waiting =
        anon &&
        (download.status == 'METADATA' ||
            (download.speedDown == 0 && download.speedUp == 0));
    final color = !anon
        ? scheme.onErrorContainer
        : waiting
        ? scheme.tertiary
        : scheme.primary;
    return Tooltip(
      message: !anon
          ? l10n.trafficDirect
          : l10n.anonTip(hops, waiting ? l10n.waitingCircuit : ''),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
        decoration: BoxDecoration(
          border: Border.all(color: !anon ? scheme.error : color),
          borderRadius: BorderRadius.circular(6),
          // « Clair » = IP exposee : badge rempli d'avertissement,
          // immediatement distinguable des lanes anonymes.
          color: !anon ? scheme.errorContainer : color.withAlpha(18),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              !anon
                  ? Icons.no_encryption_outlined
                  : waiting
                  ? Icons.shield_outlined
                  : Icons.shield,
              size: 11,
              color: color,
            ),
            const SizedBox(width: 3),
            Text(
              !anon ? l10n.badgeClear : l10n.badgeAnon(hops),
              style: TextStyle(fontSize: 10, color: color),
            ),
          ],
        ),
      ),
    );
  }
}
