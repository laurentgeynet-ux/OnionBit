// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/events.dart';
import '../api/sse_client.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../notifications/web_notify.dart';

/// Affiche un snackbar à chaque événement SSE `torrent_finished`
/// (notification `notifications.torrent_finished` Python : `infohash`,
/// `name`, `hidden`). Vit dans le shell — actif sur toutes les pages.
/// Ne rend rien.
class TorrentFinishedListener extends ConsumerWidget {
  const TorrentFinishedListener({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event == null || event.topic != EventTopics.torrentFinished) {
        return;
      }
      final name =
          (event.data['name'] as String?) ??
          (event.data['infohash'] as String?) ??
          '';
      // Notification navigateur quand l'onglet est en arrière-plan
      // (no-op desktop : le snackbar ci-dessous suffit).
      notifySystem(l10n.snackFinished, name);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Row(
            children: [
              const Icon(Icons.check_circle_outline, size: 18),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  name.isEmpty
                      ? l10n.snackFinished
                      : l10n.snackFinishedName(name),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
      );
    });
    return const SizedBox.shrink();
  }
}
