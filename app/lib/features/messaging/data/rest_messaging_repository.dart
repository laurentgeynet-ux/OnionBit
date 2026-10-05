// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import '../../../core/api/api_client.dart';
import '../../../core/config/ui_log.dart';
import '../domain/messaging_contact.dart';
import '../domain/messaging_message.dart';
import '../domain/messaging_repository.dart';
import 'messaging_dto.dart';

/// Implémentation REST du dépôt messagerie (`/api/messaging/*` —
/// extension Rust, ADR-0011). Chaque appel propage `ApiException` :
/// le 404 « messagerie desactivee » sert aussi de sonde
/// d'activation côté provider.
class RestMessagingRepository implements MessagingRepository {
  RestMessagingRepository(this._api);

  final ApiClient _api;

  @override
  Future<MessagingStats> stats() async {
    final resp = await _api.get('/messaging/stats') as Map<String, dynamic>;
    final raw = resp['stats'] as Map<String, dynamic>? ?? const {};
    return MessagingStats(
      publicKey: (resp['public_key'] as String?) ?? '',
      messagingHash: (resp['messaging_hash'] as String?) ?? '',
      counters: {
        for (final e in raw.entries) e.key: (e.value as num?)?.toInt() ?? 0,
      },
    );
  }

  @override
  Future<List<MessagingContact>> contacts() async {
    final resp = await _api.get('/messaging/contacts') as Map<String, dynamic>;
    final items = resp['contacts'] as List<dynamic>? ?? const [];
    return items
        .whereType<Map<String, dynamic>>()
        .map((c) => c.toMessagingContact())
        .toList();
  }

  @override
  Future<List<MessagingContact>> pending() async {
    final resp =
        await _api.get('/messaging/contacts/pending') as Map<String, dynamic>;
    final items = resp['contacts'] as List<dynamic>? ?? const [];
    return items
        .whereType<Map<String, dynamic>>()
        .map((c) => c.toMessagingContact())
        .toList();
  }

  @override
  Future<int> connect(String publicKey) async {
    final resp = await _api.post(
      '/messaging/contacts/connect',
      body: {'public_key': publicKey},
    ) as Map<String, dynamic>;
    return (resp['circuit_id'] as num?)?.toInt() ?? 0;
  }

  @override
  Future<void> accept(String publicKey) =>
      _api.post('/messaging/contacts/$publicKey/accept', body: {});

  @override
  Future<void> refuse(String publicKey) =>
      _api.post('/messaging/contacts/$publicKey/refuse', body: {});

  @override
  Future<void> block(String publicKey) =>
      _api.post('/messaging/contacts/$publicKey/block', body: {});

  @override
  Future<void> unblock(String publicKey) =>
      _api.delete('/messaging/contacts/$publicKey/block');

  @override
  Future<void> remove(String publicKey) =>
      _api.delete('/messaging/contacts/$publicKey');

  @override
  Future<List<MessagingMessage>> history(
    String publicKey, {
    int limit = 100,
  }) async {
    final resp = await _api.get(
      '/messaging/contacts/$publicKey/messages',
      query: {'limit': '$limit'},
    ) as Map<String, dynamic>;
    final items = resp['messages'] as List<dynamic>? ?? const [];
    return items
        .whereType<Map<String, dynamic>>()
        .map((m) => m.toMessagingMessage())
        .toList();
  }

  @override
  Future<String> send(String publicKey, String body) async {
    try {
      final resp = await _api.post(
        '/messaging/contacts/$publicKey/messages',
        body: {'body': body},
      ) as Map<String, dynamic>;
      return (resp['id'] as String?) ?? '';
    } catch (e) {
      // 404 = contact hors ligne : non livré, enregistré `failed`
      // dans l'historique (online-only — pas de file).
      uiLog('envoi messagerie en echec : $e');
      rethrow;
    }
  }

  @override
  Future<void> deleteMessage(String id) =>
      _api.delete('/messaging/messages/$id');

  @override
  Future<void> setRetention(
    String publicKey, {
    required int retentionSecs,
    bool secureDelete = false,
  }) => _api.post(
    '/messaging/contacts/$publicKey/retention',
    body: {'retention_secs': retentionSecs, 'secure_delete': secureDelete},
  );
}
