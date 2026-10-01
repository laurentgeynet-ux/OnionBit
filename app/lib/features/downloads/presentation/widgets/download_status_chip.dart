// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../../../../core/widgets/status_chip.dart';
import '../../domain/download.dart';

/// Puce de statut d'un téléchargement — mapping des statuts Python
/// (`DLSTATUS_*`) vers libellé français + tonalité.
class DownloadStatusChip extends StatelessWidget {
  const DownloadStatusChip({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final (label, tone) = _map(download);
    return StatusChip(label: label, tone: tone);
  }

  static (String, StatusTone) _map(Download d) {
    if (d.isError) return ('Erreur', StatusTone.negative);
    return switch (d.status) {
      'DOWNLOADING' => (
        'Téléchargement ${(d.progress * 100).toStringAsFixed(0)} %',
        StatusTone.positive,
      ),
      'SEEDING' => (
        'Partage ${(d.progress * 100).toStringAsFixed(0)} %',
        StatusTone.positive,
      ),
      'METADATA' => ('Métadonnées', StatusTone.warning),
      'HASHCHECKING' ||
      'WAITING_FOR_HASHCHECK' => ('Vérification', StatusTone.warning),
      'STOPPED' => ('Arrêté', StatusTone.neutral),
      _ => (d.status.isEmpty ? '—' : d.status, StatusTone.neutral),
    };
  }
}

/// Badge d'anonymat explicite : « Clair » (trafic direct) ou
/// « Anon ×N » (N sauts de circuit). Contour plein = lane anonyme
/// établie ; contour pointillé = « en attente de circuit » (le kill
/// switch bloque le trafic sans circuit).
class AnonBadge extends StatelessWidget {
  const AnonBadge({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final hops = download.hops;
    final anon = download.anonDownload && hops > 0;
    final waiting =
        anon &&
        (download.status == 'METADATA' ||
            (download.speedDown == 0 && download.speedUp == 0));
    final color = !anon
        ? scheme.outline
        : waiting
        ? scheme.tertiary
        : scheme.primary;
    return Tooltip(
      message: !anon
          ? 'Trafic direct (non anonyme)'
          : 'Anonyme : $hops saut(s)'
                '${waiting ? ' — en attente de circuit' : ''}',
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
        decoration: BoxDecoration(
          border: Border.all(color: color),
          borderRadius: BorderRadius.circular(6),
          color: anon ? color.withAlpha(18) : null,
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              !anon
                  ? Icons.public
                  : waiting
                  ? Icons.shield_outlined
                  : Icons.shield,
              size: 11,
              color: color,
            ),
            const SizedBox(width: 3),
            Text(
              !anon ? 'Clair' : 'Anon ×$hops',
              style: TextStyle(fontSize: 10, color: color),
            ),
          ],
        ),
      ),
    );
  }
}
