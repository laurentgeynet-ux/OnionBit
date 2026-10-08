// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Pièce jointe de messagerie (ADR-0019 §4) — ligne `msg_attachments`.
///
/// La trame ne transporte que le descripteur (`ih` salé, `mid`,
/// `name`, `size`) : les octets voyagent en BitTorrent anonyme après
/// acceptation explicite du destinataire (`attach_accept`).
class MessagingAttachment {
  const MessagingAttachment({
    required this.attachId,
    required this.convId,
    required this.infohash,
    required this.name,
    required this.size,
    required this.role,
    required this.state,
    required this.createdAt,
  });

  /// `attach_id` hex (16 octets) — cible de `accept`/`decline`.
  final String attachId;

  /// Conversation propriétaire (`conv_id` hex).
  final String convId;

  /// Infohash **salé** du torrent éphémère (hex 20 octets).
  final String infohash;

  /// Nom d'affichage du fichier.
  final String name;

  /// Taille en octets.
  final int size;

  /// `offer` (émise par nous — seeding en cours) | `receive`
  /// (reçue — en attente d'acceptation ou téléchargée).
  final String role;

  /// `offered | seeding | accepted | downloading | done | declined |
  /// expired` — machine d'état de l'étape 67.
  final String state;

  /// Date d'insertion locale (secondes epoch).
  final int createdAt;

  bool get isIncoming => role == 'receive';

  /// Offre entrante encore actionnable (boutons accepter/refuser).
  bool get isActionable => isIncoming && state == 'offered';
}

/// Résultat de `POST /messaging/uploads` — fichier stagé sous
/// `@state/messaging/uploads/` (TTL `upload_ttl`, jamais public).
class MessagingUpload {
  const MessagingUpload({
    required this.uploadId,
    required this.name,
    required this.size,
  });

  /// `upload_id` hex — jeton consommé par
  /// `POST /conversations/{conv}/attachments`.
  final String uploadId;
  final String name;
  final int size;
}

/// Résultat de `POST /conversations/{conv}/attachments` — offre
/// créée : torrent salé seedé anonymement + trames `attach` émises.
class AttachOfferResult {
  const AttachOfferResult({
    required this.attachId,
    required this.convId,
    required this.infohash,
    required this.name,
    required this.size,
    required this.sent,
  });

  final String attachId;
  final String convId;
  final String infohash;
  final String name;
  final int size;

  /// Nombre de trames `attach` effectivement émises (fan-out groupe
  /// — un membre hors ligne n'est pas compté, online-only).
  final int sent;
}
