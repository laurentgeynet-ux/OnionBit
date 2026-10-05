// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../domain/messaging_contact.dart';
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
  );
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
  );
}
