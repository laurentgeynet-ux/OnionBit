// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/events.dart';
import '../api/sse_client.dart';
import '../di/providers.dart';
import 'app_notification.dart';
import 'notifications_provider.dart';

/// Traduit les événements SSE `/api/events` en entrées du centre de
/// notifications (session). Vit dans le shell — actif sur toutes les
/// pages. Ne rend rien. Les snackbars restent gérés par
/// `TorrentFinishedListener`.
class NotificationsListener extends ConsumerStatefulWidget {
  const NotificationsListener({super.key});

  @override
  ConsumerState<NotificationsListener> createState() =>
      _NotificationsListenerState();
}

class _NotificationsListenerState extends ConsumerState<NotificationsListener> {
  /// Infohashes déjà signalés en erreur — évite un spam d'entrées à
  /// chaque poll `download_state_changed` (statut persistant).
  final _errorSeen = <String>{};

  void _onEvent(SseEvent event) {
    final n = switch (event.topic) {
      EventTopics.torrentFinished => AppNotification(
        title: 'Téléchargement terminé',
        message:
            (event.data['name'] as String?) ??
            (event.data['infohash'] as String?) ??
            '',
        severity: AppNotificationSeverity.success,
      ),
      EventTopics.downloadStateChanged => _downloadChanged(event),
      'tribler_exception' => AppNotification(
        title: 'Erreur daemon',
        message: '${event.data['error'] ?? ''}',
        severity: AppNotificationSeverity.error,
      ),
      'low_space' => AppNotification(
        title: 'Espace disque faible',
        message: 'Le dossier de téléchargement approche la saturation.',
        severity: AppNotificationSeverity.warning,
      ),
      'tribler_new_version' => AppNotification(
        title: 'Mise à jour disponible',
        message: 'Version ${event.data['version'] ?? ''} disponible.',
        severity: AppNotificationSeverity.info,
      ),
      _ => null,
    };
    if (n != null) ref.read(notificationsProvider.notifier).push(n);
  }

  /// `download_state_changed` porte un `DownloadInfo` complet — on ne
  /// notifie que le passage en erreur (dédupliqué par infohash).
  AppNotification? _downloadChanged(SseEvent event) {
    final status = event.data['status'] as String?;
    final infohash = event.data['infohash'] as String? ?? '';
    if (status == 'STOPPED_ON_ERROR') {
      if (!_errorSeen.add(infohash)) return null;
      return AppNotification(
        title: 'Téléchargement en erreur',
        message:
            (event.data['name'] as String?) ??
            (event.data['error'] as String?) ??
            infohash,
        severity: AppNotificationSeverity.error,
      );
    }
    // L'infohash redevient notifiable une fois sorti de l'erreur.
    _errorSeen.remove(infohash);
    return null;
  }

  @override
  Widget build(BuildContext context) {
    ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event != null) _onEvent(event);
    });
    return const SizedBox.shrink();
  }
}
