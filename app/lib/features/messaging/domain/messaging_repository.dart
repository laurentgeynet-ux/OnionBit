// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'messaging_attachment.dart';
import 'messaging_contact.dart';
import 'messaging_conversation.dart';
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

  /// `GET /messaging/vault/export` — coffre de contacts chiffré pour
  /// notre propre identité (blob `OBV1…` hex, ADR-0015/ADR-0016).
  /// Illisible sans la clé privée — pseudonymes inclus.
  Future<String> vaultExport();

  /// `POST /messaging/vault/import` — restaure les contacts d'un
  /// coffre exporté par la même identité. Renvoie le nombre de
  /// contacts restaurés.
  Future<int> vaultImport(String vaultHex);

  // ── ADR-0019 : conversations, groupes, pièces jointes ──────

  /// `GET /messaging/conversations` — conversations directes et
  /// groupes (non-lus + dernier horodatage).
  Future<List<MessagingConversation>> conversations();

  /// `GET /messaging/conversations/direct/{pk}` — `conv_id`
  /// déterministe de la conv directe avec `pk` (peut ne pas encore
  /// exister en base).
  Future<String> directConversation(String publicKey);

  /// `GET /conversations/{conv}/messages` — historique borné de la
  /// conversation (le plus récent d'abord).
  Future<List<MessagingMessage>> convHistory(
    String convId, {
    int limit = 100,
  });

  /// `POST /conversations/{conv}/messages` — envoi dans la conv :
  /// `send` au contact en direct, `group_send` en groupe.
  Future<String> convSend(String convId, String body);

  /// `POST /conversations/{conv}/read` — marque la conv lue.
  Future<void> convMarkRead(String convId);

  /// `DELETE /conversations/{conv}` — suppression réelle (cascade).
  Future<void> convDelete(String convId);

  /// `POST /messaging/groups` — crée un groupe : `members` = clés
  /// hex de contacts actifs invités. Renvoie le `conv_id`.
  Future<String> groupCreate(String name, List<String> members);

  /// `GET /groups/{conv}/members` — roster du groupe.
  Future<List<MessagingMember>> groupMembers(String convId);

  /// `POST /groups/{conv}/invite` — invite un contact actif.
  Future<void> groupInvite(String convId, String publicKey);

  /// `POST /groups/{conv}/accept|decline|leave`.
  Future<void> groupAccept(String convId);
  Future<void> groupDecline(String convId);
  Future<void> groupLeave(String convId);

  /// `POST /messaging/uploads` JSON `{path}` — stage un fichier
  /// local du daemon (desktop ; `@private` refusé).
  Future<MessagingUpload> uploadPath(String path, {String? name});

  /// `POST /messaging/uploads` octets + `?name=` — staging direct
  /// (web, ou octets déjà en mémoire).
  Future<MessagingUpload> uploadBytes(String name, List<int> bytes);

  /// `POST /conversations/{conv}/attachments` — offre la pièce
  /// jointe (`uploadId` ou `path` local) : torrent salé + seed
  /// anonyme + trames `attach` en fan-out.
  Future<AttachOfferResult> convAttach(
    String convId, {
    String? uploadId,
    String? path,
    String? name,
  });

  /// `GET /conversations/{conv}/attachments` — offres et réceptions.
  Future<List<MessagingAttachment>> convAttachments(String convId);

  /// `POST /attachments/{id}/accept` — accepte l'offre : download
  /// anonyme de l'infohash salé. `area` : `public|private`, `dir` :
  /// spec `@…` ou chemin. `ApiException(409)` = `identity_locked`.
  Future<String> attachAccept(String attachId, {String? area, String? dir});

  /// `POST /attachments/{id}/decline` — refuse l'offre.
  Future<void> attachDecline(String attachId);
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
