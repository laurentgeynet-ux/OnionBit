// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Conversation de messagerie (ADR-0019) — abstraction unifiée
/// direct/groupe adressée par `conv_id` (16 octets hex).
///
/// - `direct` : `conv_id` dérivé déterministement des deux clés —
///   `peer` est renseigné, `alias` reprend le pseudonyme du contact.
/// - `group` : `conv_id` aléatoire, `name` porté par les `gctl`.
class MessagingConversation {
  const MessagingConversation({
    required this.convId,
    required this.kind,
    required this.name,
    required this.state,
    required this.createdAt,
    required this.unread,
    required this.lastTs,
    this.peer,
    this.alias = '',
  });

  /// `conv_id` hex (16 octets) — adresse toutes les routes
  /// `/messaging/conversations/{conv}/…`.
  final String convId;

  /// `direct | group`.
  final String kind;

  /// Nom du groupe (vide en direct — l'UI affiche alors l'alias).
  final String name;

  /// `active | invited | left` — `invited` = invitation reçue non
  /// répondue (bannière accepter/décliner dans la conversation).
  final String state;

  /// Date de création locale (secondes epoch).
  final int createdAt;

  /// Non-lus persistés (`msg_conversations.unread`).
  final int unread;

  /// Horodatage du dernier message connu (0 = aucun).
  final int lastTs;

  /// `pk_bin` hex du correspondant — conversations `direct` seules.
  final String? peer;

  /// Pseudonyme du correspondant (direct) — vide sinon.
  final String alias;

  bool get isGroup => kind == 'group';
  bool get isInvited => state == 'invited';

  /// Libellé affiché : alias > nom de groupe > clé abrégée.
  String get displayName {
    if (alias.isNotEmpty) return alias;
    if (name.isNotEmpty) return name;
    final ref = peer ?? convId;
    return ref.length > 12 ? '${ref.substring(0, 12)}…' : ref;
  }
}

/// Membre du roster d'un groupe (`GET /messaging/groups/{conv}/members`).
class MessagingMember {
  const MessagingMember({
    required this.memberPk,
    required this.addedBy,
    required this.state,
    required this.joinedAt,
    this.alias = '',
  });

  /// `pk_bin` hex du membre.
  final String memberPk;

  /// `pk_bin` hex de l'invitant (`added_by` — confiance transitive
  /// affichable : « invité par … »).
  final String addedBy;

  /// `invited | member | left` (état `msg_members.state`).
  final String state;

  /// Horodatage d'adhésion (secondes epoch).
  final int joinedAt;

  /// Pseudonyme local du membre si c'est aussi un contact — vide
  /// pour les pairs confinés `scope='group'` (jamais contacts).
  final String alias;

  bool get isActive => state == 'member' || state == 'active';

  /// Libellé affiché : alias > clé abrégée.
  String get displayName =>
      alias.isNotEmpty ? alias : _short(memberPk);

  static String _short(String pk) =>
      pk.length > 12 ? '${pk.substring(0, 12)}…' : pk;
}
