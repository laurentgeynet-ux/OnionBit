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

/// Pastille d'anonymat — honnête : plein = lane anonyme, l'état « en
/// attente de circuit » relève du statut du téléchargement (le kill
/// switch bloque le trafic sans circuit).
class AnonBadge extends StatelessWidget {
  const AnonBadge({super.key, required this.download});

  final Download download;

  @override
  Widget build(BuildContext context) {
    if (!download.anonDownload) return const SizedBox(width: 24);
    final scheme = Theme.of(context).colorScheme;
    final waiting =
        download.status == 'METADATA' ||
        (download.speedDown == 0 && download.speedUp == 0);
    return Tooltip(
      message:
          '${download.hops} saut(s)${waiting ? ' — en attente de circuit' : ''}',
      child: Icon(
        waiting ? Icons.shield_outlined : Icons.shield,
        size: 18,
        color: waiting ? scheme.tertiary : scheme.primary,
      ),
    );
  }
}
