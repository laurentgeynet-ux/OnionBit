// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Canal suivi — forme d'un objet de `GET /api/channels`
/// (`channels_endpoint` : un objet par `(public_key, origin_id)`).
class Channel {
  const Channel({
    required this.publicKey,
    required this.id,
    required this.name,
    required this.subscribed,
    required this.numEntries,
    this.onion,
    this.onionVerified = false,
    this.planes = const ['onionbit'],
  });

  /// Clé Ed25519 du curateur (hex, 64 octets).
  final String publicKey;

  /// `origin_id` de la racine du canal (0 par convention OnionBit).
  final int id;

  /// Titre de la racine `COLLECTION_NODE` — vide tant que la racine
  /// n'a pas été synchronisée (placeholder créé au subscribe).
  final String name;

  /// Toujours `true` pour les canaux listés (le backend ne retourne
  /// que les abonnements) ; gardé pour la forme Python.
  final bool subscribed;

  /// Entrées `CHANNEL_TORRENT` actives persistées localement.
  final int numEntries;

  /// Adresse `.onion` du curateur (ADR-0029) — liaison explicite
  /// (`peer_onions`), jamais inférée ; `null` sans association.
  final String? onion;

  /// `true` quand la liaison `.onion` est signée par le pair.
  final bool onionVerified;

  /// Plans d'adressage du canal : `["onionbit"]` ou
  /// `["onionbit","onion"]` quand le curateur publie un `.onion`.
  final List<String> planes;

  /// `.onion` associé au curateur — raccourci de lecture.
  bool get hasOnion => onion != null && onion!.isNotEmpty;
}
