// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'messaging_contact.dart';
import 'messaging_message.dart';

/// Dépôt messagerie — surface API de l'UI (l'app ne touche jamais
/// le core directement : tout passe par `/api/messaging/*`).
abstract class MessagingRepository {
  /// `GET /messaging/stats` — clé publique locale + hash de
  /// présence + compteurs de drops. Jette `ApiException(404)`
  /// quand la messagerie est désactivée.
  Future<MessagingStats> stats();

  /// `GET /messaging/contacts` — contacts liés au service
  /// (état de consentement + circuit éventuel).
  Future<List<MessagingContact>> contacts();

  /// `GET /messaging/contacts/pending` — demandes en attente.
  Future<List<MessagingContact>> pending();

  /// `POST /messaging/contacts/connect` — résout les points
  /// d'introduction puis lie un circuit e2e.
  Future<int> connect(String publicKey);

  /// `POST …/accept` — accorde le consentement.
  Future<void> accept(String publicKey);

  /// `POST …/refuse` — refuse (trame `reject`, contact oublié).
  Future<void> refuse(String publicKey);

  /// `POST/DELETE …/block` — bloque / débloque.
  Future<void> block(String publicKey);
  Future<void> unblock(String publicKey);

  /// `DELETE /messaging/contacts/{pk}` — oublie le contact.
  Future<void> remove(String publicKey);

  /// `POST …/alias` — pseudonyme local (`''` = effacer).
  Future<void> setAlias(String publicKey, String alias);

  /// `GET …/messages` — historique borné (le plus récent d'abord).
  Future<List<MessagingMessage>> history(String publicKey, {int limit = 100});

  /// `POST …/messages` — envoi ; `ApiException(404)` si le contact
  /// est hors ligne (enregistré `failed` — jamais de file).
  Future<String> send(String publicKey, String body);

  /// `DELETE /messaging/messages/{id}` — suppression réelle.
  Future<void> deleteMessage(String id);

  /// `POST …/retention` — rétention des messages du contact.
  Future<void> setRetention(
    String publicKey, {
    required int retentionSecs,
    bool secureDelete = false,
  });
}

/// Identité locale + compteurs exposés par `/messaging/stats`.
class MessagingStats {
  const MessagingStats({
    required this.publicKey,
    required this.messagingHash,
    required this.counters,
  });

  /// Clé publique de l'identité daemon (hex) — adresse des contacts.
  final String publicKey;

  /// `messaging_hash` de notre swarm de présence (hex).
  final String messagingHash;

  /// Compteurs de drops du demux (codec/rate/consent/…).
  final Map<String, int> counters;
}
