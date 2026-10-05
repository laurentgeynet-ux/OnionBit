// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/api/sse_client.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_messaging_repository.dart';
import '../../domain/messaging_contact.dart';
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
  });
});
