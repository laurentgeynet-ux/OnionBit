// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../domain/messaging_attachment.dart';
import '../domain/messaging_contact.dart';
import '../domain/messaging_conversation.dart';
import '../domain/messaging_message.dart';

/// Parse JSON → entités messagerie (`/api/messaging/*`).
extension MessagingContactJson on Map<String, dynamic> {
  MessagingContact toMessagingContact() => MessagingContact(
    publicKey: (this['public_key'] as String?) ?? '',
    state: switch (this['state'] as String? ?? '') {
      'active' => MessagingContactState.active,
      'pending' => MessagingContactState.pending,
      'blocked' => MessagingContactState.blocked,
      _ => MessagingContactState.unknown,
    },
    circuitId: (this['circuit_id'] as num?)?.toInt(),
    pendingSinceSecs: (this['pending_since_secs'] as num?)?.toInt(),
    alias: (this['alias'] as String?) ?? '',
    link: switch (this['link'] as String? ?? '') {
      'bound' => MessagingLinkState.bound,
      'connecting' => MessagingLinkState.connecting,
      'failed' => MessagingLinkState.failed,
      _ => MessagingLinkState.none,
    },
    onion: this['onion'] as String?,
    onionVerified: this['onion_verified'] as bool? ?? false,
    planes: _planes(this['planes'], contact: true),
  );
}

/// Plans d'adressage (`planes` du DTO — ADR-0029). `"onionbit"`
/// est la valeur de repli d'un contact (sa clé IPv8 est son
/// identité de base) ; les conversations sans pair connu restent
/// à liste vide.
List<String> _planes(Object? raw, {bool contact = false}) {
  final planes = [
    for (final p in (raw as List<dynamic>?) ?? const [])
      if (p is String && p.isNotEmpty) p,
  ];
  if (planes.isEmpty && contact) return const ['onionbit'];
  return planes;
}

extension MessagingMessageJson on Map<String, dynamic> {
  MessagingMessage toMessagingMessage() => MessagingMessage(
    id: (this['id'] as String?) ?? '',
    direction: (this['direction'] as String?) ?? '',
    seq: (this['seq'] as num?)?.toInt() ?? 0,
    ts: (this['ts'] as num?)?.toInt() ?? 0,
    body: (this['body'] as String?) ?? '',
    status: (this['status'] as String?) ?? '',
    createdAt: (this['created_at'] as num?)?.toInt() ?? 0,
    authorPk: this['author_pk'] as String?,
    mid: this['mid'] as String?,
    plane: (this['plane'] as String?) ?? 'ipv8',
  );
}

/// Parse ADR-0019 : conversations, membres de groupe, pieces
/// jointes (`/messaging/conversations`, `/groups`, `/attachments`).
extension MessagingConversationJson on Map<String, dynamic> {
  MessagingConversation toMessagingConversation() => MessagingConversation(
    convId: (this['conv_id'] as String?) ?? '',
    kind: (this['kind'] as String?) ?? 'direct',
    name: (this['name'] as String?) ?? '',
    state: (this['state'] as String?) ?? 'active',
    createdAt: (this['created_at'] as num?)?.toInt() ?? 0,
    unread: (this['unread'] as num?)?.toInt() ?? 0,
    lastTs: (this['last_ts'] as num?)?.toInt() ?? 0,
    peer: this['peer'] as String?,
    alias: (this['alias'] as String?) ?? '',
    onion: this['onion'] as String?,
    planes: _planes(this['planes']),
  );

  MessagingMember toMessagingMember() => MessagingMember(
    memberPk: (this['member_pk'] as String?) ?? '',
    addedBy: (this['added_by'] as String?) ?? '',
    state: (this['state'] as String?) ?? '',
    joinedAt: (this['joined_at'] as num?)?.toInt() ?? 0,
    alias: (this['alias'] as String?) ?? '',
  );

  MessagingAttachment toMessagingAttachment() => MessagingAttachment(
    attachId: (this['attach_id'] as String?) ?? '',
    convId: (this['conv_id'] as String?) ?? '',
    infohash: (this['infohash'] as String?) ?? '',
    name: (this['name'] as String?) ?? '',
    size: (this['size'] as num?)?.toInt() ?? 0,
    role: (this['role'] as String?) ?? '',
    state: (this['state'] as String?) ?? '',
    createdAt: (this['created_at'] as num?)?.toInt() ?? 0,
  );

  MessagingUpload toMessagingUpload() => MessagingUpload(
    uploadId: (this['upload_id'] as String?) ?? '',
    name: (this['name'] as String?) ?? '',
    size: (this['size'] as num?)?.toInt() ?? 0,
  );

  AttachOfferResult toAttachOfferResult() => AttachOfferResult(
    attachId: (this['attach_id'] as String?) ?? '',
    convId: (this['conv_id'] as String?) ?? '',
    infohash: (this['infohash'] as String?) ?? '',
    name: (this['name'] as String?) ?? '',
    size: (this['size'] as num?)?.toInt() ?? 0,
    sent: (this['sent'] as num?)?.toInt() ?? 0,
  );
}
