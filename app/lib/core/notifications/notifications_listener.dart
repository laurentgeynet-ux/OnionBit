// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../l10n/app_localizations.dart' show AppLocalizations;
import '../api/events.dart';
import '../api/sse_client.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
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

  void _onEvent(AppLocalizations l10n, SseEvent event) {
    final n = switch (event.topic) {
      EventTopics.torrentFinished => AppNotification(
        title: l10n.notifDownloadFinished,
        message:
            (event.data['name'] as String?) ??
            (event.data['infohash'] as String?) ??
            '',
        severity: AppNotificationSeverity.success,
      ),
      EventTopics.downloadStateChanged => _downloadChanged(l10n, event),
      'tribler_exception' => AppNotification(
        title: l10n.notifDaemonError,
        message: '${event.data['error'] ?? ''}',
        severity: AppNotificationSeverity.error,
      ),
      'low_space' => AppNotification(
        title: l10n.notifLowSpace,
        message: l10n.notifLowSpaceMsg,
        severity: AppNotificationSeverity.warning,
      ),
      'tribler_new_version' => AppNotification(
        title: l10n.notifNewVersion,
        message: l10n.notifNewVersionMsg('${event.data['version'] ?? ''}'),
        severity: AppNotificationSeverity.info,
      ),
      _ => null,
    };
    if (n != null) ref.read(notificationsProvider.notifier).push(n);
  }

  /// `download_state_changed` porte un `DownloadInfo` complet — on ne
  /// notifie que le passage en erreur (dédupliqué par infohash).
  AppNotification? _downloadChanged(AppLocalizations l10n, SseEvent event) {
    final status = event.data['status'] as String?;
    final infohash = event.data['infohash'] as String? ?? '';
    if (status == 'STOPPED_ON_ERROR') {
      if (!_errorSeen.add(infohash)) return null;
      return AppNotification(
        title: l10n.notifDownloadError,
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
    // Capturé avant le listener — les chaînes de notification suivent
    // la locale active au moment de l'événement.
    final l10n = context.l10n;
    ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event != null) _onEvent(l10n, event);
    });
    return const SizedBox.shrink();
  }
}
