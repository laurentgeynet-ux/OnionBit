// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/api/sse_client.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_messaging_repository.dart';
import '../../domain/messaging_attachment.dart';
import '../../domain/messaging_contact.dart';
import '../../domain/messaging_conversation.dart';
import '../../domain/messaging_message.dart';
import '../../domain/messaging_repository.dart';

/// Dépôt messagerie — la page n'appelle jamais l'`ApiClient`
/// directement, comme les autres features.
final messagingRepositoryProvider = Provider<MessagingRepository>(
  (ref) => RestMessagingRepository(ref.watch(apiClientProvider)),
);

/// Sonde d'activation : `GET /messaging/stats` répond 404 quand
/// `enable_messaging` est off — la page affiche alors l'indication
/// d'activation au lieu d'une erreur.
final messagingEnabledProvider = FutureProvider<bool>((ref) async {
  try {
    await ref.watch(messagingRepositoryProvider).stats();
    return true;
  } on ApiException catch (e) {
    if (e.statusCode == 404) return false;
    rethrow;
  }
});

/// Identité locale (`public_key` hex = adresse à donner aux
/// contacts) — `null` si messagerie désactivée.
final messagingStatsProvider = FutureProvider<MessagingStats?>((ref) async {
  if (!(await ref.watch(messagingEnabledProvider.future))) return null;
  return ref.watch(messagingRepositoryProvider).stats();
});

/// Contacts liés au service (état + circuit éventuel).
final messagingContactsProvider = FutureProvider<List<MessagingContact>>((
  ref,
) async {
  if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
  return ref.watch(messagingRepositoryProvider).contacts();
});

/// Demandes de consentement en attente (section haute de la page).
final messagingPendingProvider = FutureProvider<List<MessagingContact>>((
  ref,
) async {
  if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
  return ref.watch(messagingRepositoryProvider).pending();
});

/// Contact sélectionné dans la conversation (clé publique hex).
final selectedContactProvider =
    NotifierProvider<SelectedContactNotifier, String?>(
      SelectedContactNotifier.new,
    );

class SelectedContactNotifier extends Notifier<String?> {
  @override
  String? build() => null;

  /// Sélectionne / désélectionne (`null`) un contact.
  void set(String? publicKey) => state = publicKey;
}

/// Historique borné du contact (`pk` en hex), le plus récent
/// d'abord — inversé à l'affichage.
final messagingHistoryProvider = FutureProvider.autoDispose
    .family<List<MessagingMessage>, String>((ref, publicKey) async {
      if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
      return ref.watch(messagingRepositoryProvider).history(publicKey);
    });

// ── ADR-0019 : conversations, onglets, groupes, pieces ─────────

/// Conversations (directes + groupes) — `unread`/`last_ts` servent
/// le tri et les badges de la liste.
final messagingConversationsProvider =
    FutureProvider<List<MessagingConversation>>((ref) async {
      if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
      return ref.watch(messagingRepositoryProvider).conversations();
    });

/// Onglet de conversation ouvert : `convId` + `peer` (clé du
/// correspondant quand la conv directe n'a pas encore de ligne
/// persistée — purement affichage).
typedef OpenConvTab = ({String convId, String? peer});

/// Onglets de conversation ouverts (UI-only, ADR-0019 §5 : les
/// onglets ne sont pas persistés — ce sont des vues sur `conv_id`).
final openConversationsProvider =
    NotifierProvider<OpenConversationsNotifier, List<OpenConvTab>>(
      OpenConversationsNotifier.new,
    );

class OpenConversationsNotifier extends Notifier<List<OpenConvTab>> {
  @override
  List<OpenConvTab> build() => const [];

  /// Ouvre (ou sélectionne) l'onglet de `convId`.
  void open(String convId, {String? peer}) {
    if (!state.any((t) => t.convId == convId)) {
      state = [...state, (convId: convId, peer: peer)];
    }
    ref.read(selectedConversationProvider.notifier).set(convId);
  }

  /// Ferme l'onglet — si c'était le sélectionné, bascule sur le
  /// voisin le plus proche.
  void close(String convId) {
    final i = state.indexWhere((t) => t.convId == convId);
    if (i < 0) return;
    final next = [...state]..removeAt(i);
    state = next;
    if (ref.read(selectedConversationProvider) == convId) {
      ref
          .read(selectedConversationProvider.notifier)
          .set(next.isEmpty ? null : next[i.clamp(0, next.length - 1)].convId);
    }
  }
}

/// Conversation sélectionnée (`conv_id` hex) — l'onglet actif.
final selectedConversationProvider =
    NotifierProvider<SelectedConversationNotifier, String?>(
      SelectedConversationNotifier.new,
    );

class SelectedConversationNotifier extends Notifier<String?> {
  @override
  String? build() => null;

  /// Sélectionne / désélectionne (`null`) une conversation.
  void set(String? convId) => state = convId;
}

/// Historique borné d'une conversation (`conv_id` hex), le plus
/// récent d'abord — marque lue à l'ouverture (les non-lus sont
/// consommés par l'affichage).
final convHistoryProvider = FutureProvider.autoDispose
    .family<List<MessagingMessage>, String>((ref, convId) async {
      if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
      final msgs = await ref
          .watch(messagingRepositoryProvider)
          .convHistory(convId);
      // Marque lu dès que l'historique est rendu — l'invalidation
      // des conversations rafraîchit le badge `unread`.
      await ref.read(messagingRepositoryProvider).convMarkRead(convId);
      ref.invalidate(messagingConversationsProvider);
      return msgs;
    });

/// Roster d'un groupe (`conv_id` hex) — membres `member`/`invited`/
/// `left` avec `added_by` (« invité par … »).
final groupMembersProvider = FutureProvider.autoDispose
    .family<List<MessagingMember>, String>((ref, convId) async {
      if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
      return ref.watch(messagingRepositoryProvider).groupMembers(convId);
    });

/// Pièces jointes d'une conversation (offres émises + reçues) —
/// rafraîchi par le SSE `messaging_attach` et après accept/refus.
final convAttachmentsProvider = FutureProvider.autoDispose
    .family<List<MessagingAttachment>, String>((ref, convId) async {
      if (!(await ref.watch(messagingEnabledProvider.future))) return const [];
      return ref.watch(messagingRepositoryProvider).convAttachments(convId);
    });

/// Flux SSE dédié `/api/messaging/events` — ouvert uniquement quand
/// la messagerie est activée (sinon le endpoint répond 404 et le
/// backoff SSE spammerait). `null`-safe : vide si désactivée.
final messagingEventsProvider = StreamProvider<SseEvent>((ref) async* {
  if (!(await ref.watch(messagingEnabledProvider.future))) return;
  final client = SseClient(
    ref.watch(appConfigProvider),
    path: '/messaging/events',
  );
  client.start();
  ref.onDispose(client.dispose);
  yield* client.events;
});

/// Pont SSE → invalidation : sur réception d'un événement
/// messagerie, les données se re-synchronisent par pull (le flux
/// est un indice de fraîcheur, jamais une source fiable —
/// convention `sse_client.dart`).
final messagingEventsBridgeProvider = Provider<void>((ref) {
  ref.listen(messagingEventsProvider, (_, next) {
    final ev = next.value;
    if (ev == null) return;
    ref.invalidate(messagingContactsProvider);
    ref.invalidate(messagingPendingProvider);
    final selected = ref.read(selectedContactProvider);
    if (selected != null) {
      ref.invalidate(messagingHistoryProvider(selected));
    }
    // ADR-0019 : événements conversation/groupe/pièce jointe —
    // `messaging_conv` et `messaging_attach` portent `conv_id`,
    // les autres re-synchronisent la liste entière.
    final conv = (ev.data['conv_id'] as String?) ?? '';
    switch (ev.topic) {
      case 'messaging_conv':
        ref.invalidate(messagingConversationsProvider);
        if (conv.isNotEmpty) {
          ref.invalidate(convHistoryProvider(conv));
        }
      case 'messaging_attach':
        if (conv.isNotEmpty) {
          ref.invalidate(convAttachmentsProvider(conv));
          ref.invalidate(convHistoryProvider(conv));
        }
      case 'messaging_group_invite':
        ref.invalidate(messagingConversationsProvider);
        if (conv.isNotEmpty) {
          ref.invalidate(groupMembersProvider(conv));
        }
      default:
        break;
    }
    // La conversation sélectionnée consomme aussi ses non-lus.
    final selConv = ref.read(selectedConversationProvider);
    if (selConv != null) {
      ref.invalidate(messagingConversationsProvider);
    }
  });
});
