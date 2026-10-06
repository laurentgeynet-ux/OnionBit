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

/// État de liaison e2e d'un contact — distinct du consentement
/// ([`MessagingContactState`]) : un contact consenti peut être sans
/// circuit. Dérivé côté daemon (`link` de `GET /contacts`).
enum MessagingLinkState {
  /// Circuit e2e lié — messages émissibles.
  bound,

  /// Liaison en cours (connect explicite ou tentative automatique
  /// du tunnel).
  connecting,

  /// Dernière tentative échouée — réessayer via « Reconnecter ».
  failed,

  /// Aucune liaison ni tentative connue.
  none,
}

/// Contact de messagerie e2e tel que `GET /api/messaging/contacts`
/// le rend (extension Rust — pas de parité Python).
class MessagingContact {
  const MessagingContact({
    required this.publicKey,
    required this.state,
    this.circuitId,
    this.pendingSinceSecs,
    this.alias = '',
    this.link = MessagingLinkState.none,
  });

  /// Clé publique du contact en hex (`pk_bin` de l'identité daemon).
  final String publicKey;

  /// État de consentement courant.
  final MessagingContactState state;

  /// `circuit_id` e2e actuellement lié (`null` = hors ligne).
  final int? circuitId;

  /// Ancienneté d'une demande `pending` en secondes.
  final int? pendingSinceSecs;

  /// Pseudonyme local (`''` = aucun — repli sur la clé abrégée).
  final String alias;

  /// État de liaison e2e courant (indicateur du point de statut).
  final MessagingLinkState link;

  /// Raccourci lisible de la clé (8 premiers caractères hex).
  String get shortKey =>
      publicKey.length > 8 ? publicKey.substring(0, 8) : publicKey;

  /// Nom affiché : pseudonyme s'il existe, sinon clé abrégée.
  String get displayName => alias.isNotEmpty ? alias : shortKey;
}
