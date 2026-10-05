// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// État de consentement d'un contact messagerie (ADR-0011).
enum MessagingContactState {
  /// Consenti : trames livrées à l'application.
  active,

  /// En attente de décision utilisateur (borne + TTL côté daemon).
  pending,

  /// Bloqué : trames ignorées, circuits détruits.
  blocked,

  /// État inconnu côté daemon (contact non encore enregistré —
  /// ex. déstinataire d'un envoi sortant jamais lié).
  unknown,
}

/// Contact de messagerie e2e tel que `GET /api/messaging/contacts`
/// le rend (extension Rust — pas de parité Python).
class MessagingContact {
  const MessagingContact({
    required this.publicKey,
    required this.state,
    this.circuitId,
    this.pendingSinceSecs,
  });

  /// Clé publique du contact en hex (`pk_bin` de l'identité daemon).
  final String publicKey;

  /// État de consentement courant.
  final MessagingContactState state;

  /// `circuit_id` e2e actuellement lié (`null` = hors ligne).
  final int? circuitId;

  /// Ancienneté d'une demande `pending` en secondes.
  final int? pendingSinceSecs;

  /// Raccourci lisible de la clé (8 premiers caractères hex).
  String get shortKey =>
      publicKey.length > 8 ? publicKey.substring(0, 8) : publicKey;
}
