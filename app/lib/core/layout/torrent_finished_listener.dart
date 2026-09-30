import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/events.dart';
import '../api/sse_client.dart';
import '../di/providers.dart';

/// Affiche un snackbar à chaque événement SSE `torrent_finished`
/// (notification `notifications.torrent_finished` Python : `infohash`,
/// `name`, `hidden`). Vit dans le shell — actif sur toutes les pages.
/// Ne rend rien.
class TorrentFinishedListener extends ConsumerWidget {
  const TorrentFinishedListener({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
      final event = next.value;
      if (event == null || event.topic != EventTopics.torrentFinished) {
        return;
      }
      final name = (event.data['name'] as String?) ??
          (event.data['infohash'] as String?) ??
          '';
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Row(
            children: [
              const Icon(Icons.check_circle_outline, size: 18),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  name.isEmpty
                      ? 'Téléchargement terminé'
                      : '« $name » terminé — seed en cours',
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
