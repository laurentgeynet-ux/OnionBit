// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Modèles de l'explorateur de zone privée (ADR-0027, étape 110) —
/// vues déchiffrées du catalogue `manifest.obm` servies par
/// `GET /api/private*` (endpoint réservé au titulaire).
library;

/// Entrée du manifeste privé — une ligne `GET /api/private`.
class PrivateEntry {
  const PrivateEntry({
    required this.infohash,
    required this.name,
    required this.destination,
    required this.paused,
    required this.timeAdded,
  });

  /// Infohash réel (hex 40) — aussi la `key` des endpoints
  /// `files`/`export` (l'API accepte indifféremment la `row_key`).
  final String infohash;

  /// Nom réel du contenu (vide pour une entrée reconstruite).
  final String name;

  /// Spec `@private/temp|downloads` de la sous-racine courante.
  final String destination;

  /// Entrée ajoutée en pause.
  final bool paused;

  /// Date d'ajout (secondes Unix).
  final int timeAdded;
}

/// Fichier d'une entrée privée — `GET /api/private/{key}/files`.
class PrivateFileEntry {
  const PrivateFileEntry({
    required this.index,
    required this.path,
    required this.length,
  });

  /// Position dans la liste (clé `files` de l'export).
  final int index;

  /// Chemin relatif en clair (`a/b/c.bin`, jamais le nom HMAC).
  final String path;

  /// Taille logique en clair.
  final int length;
}

/// Catalogue déchiffré `GET /api/private` — `state` :
/// `locked` (identité non résolue), `guest` (session éphémère) ou
/// `mounted` (explorateur utilisable).
class PrivateZoneCatalog {
  const PrivateZoneCatalog({
    required this.state,
    required this.entries,
    required this.orphanGroups,
    required this.orphanBitv,
  });

  final String state;
  final List<PrivateEntry> entries;

  /// Groupes `.obd` orphelins rapportés au montage — non
  /// exportables (sceau `scan_ct` illisible → clé non dérivable),
  /// purgés via `DELETE /api/private/orphans`.
  final int orphanGroups;

  /// `.bitv` fastresume hors catalogue.
  final int orphanBitv;
}

/// Bilan d'un `POST /api/private/{key}/export`.
class PrivateExportResult {
  const PrivateExportResult({required this.exported, required this.bytes});

  final int exported;
  final int bytes;
}
