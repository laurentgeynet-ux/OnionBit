// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Message de l'historique (`GET /api/messaging/contacts/{pk}/messages`).
///
/// `status` reflète la livraison v1 : `sent` → `acked` quand l'ACK
/// applicatif revient, `failed` quand le contact est hors ligne
/// (online-only — ADR-0011 : jamais de file d'attente).
class MessagingMessage {
  const MessagingMessage({
    required this.id,
    required this.direction,
    required this.seq,
    required this.ts,
    required this.body,
    required this.status,
    required this.createdAt,
    this.authorPk,
    this.mid,
  });

  /// `id` de trame en hex (16 octets — cible des ACKs et de la
  /// suppression `DELETE /api/messaging/messages/{id}`).
  final String id;

  /// `in` (reçu) ou `out` (émis).
  final String direction;

  /// `seq` de trame (compteur anti-replay du contact).
  final int seq;

  /// Horodatage émetteur de la trame (secondes epoch).
  final int ts;

  /// Corps applicatif (UTF-8 — `from_utf8_lossy` côté daemon).
  final String body;

  /// `received | sent | acked | failed`.
  final String status;

  /// Date d'insertion locale (secondes epoch).
  final int createdAt;

  /// `pk_bin` hex de l'auteur — groupe uniquement (v2, ADR-0019) ;
  /// `null` en direct v1 (l'auteur est le contact).
  final String? authorPk;

  /// `mid` hex de regroupement (v2 : messages/attaches d'un même
  /// geste) — `null` en v1.
  final String? mid;

  bool get isOutgoing => direction == 'out';
  bool get isFailed => status == 'failed';
}
