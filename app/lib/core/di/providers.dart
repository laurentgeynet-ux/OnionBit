import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/api_client.dart';
import '../api/sse_client.dart';
import '../config/app_config.dart';
import '../config/connection_settings.dart';

/// Configuration de connexion effective : persistée via
/// `connectionSettingsProvider` (surchargeable pour les tests).
final appConfigProvider = Provider<AppConfig>(
  (ref) => ref.watch(connectionSettingsProvider).value ?? const AppConfig(),
);

/// Client REST unique.
final apiClientProvider = Provider<ApiClient>((ref) {
  final client = ApiClient(ref.watch(appConfigProvider));
  ref.onDispose(client.close);
  return client;
});

/// Flux SSE `/api/events` — démarré une fois, partagé par broadcast.
final sseClientProvider = Provider<SseClient>((ref) {
  final client = SseClient(ref.watch(appConfigProvider));
  client.start();
  ref.onDispose(client.dispose);
  return client;
});

/// État du flux SSE (barre d'état « connecté au daemon »).
final sseConnectedProvider = StreamProvider<bool>((ref) {
  return ref.watch(sseClientProvider).connected;
});

/// Événements du daemon — chaque feature filtre les topics qui
/// l'intéressent et invalide ses données en conséquence.
final daemonEventsProvider = StreamProvider<SseEvent>((ref) {
  return ref.watch(sseClientProvider).events;
});

/// Battement périodique pour les providers à sondage (compteurs de
/// circuits, statistiques…).
final tickProvider = StreamProvider.autoDispose.family<int, Duration>(
  (ref, period) => Stream.periodic(period, (i) => i),
);

/// Requête de recherche globale (champ de la barre du haut) — la page
/// `/search` la consomme ; vide = afficher les torrents populaires.
final searchQueryProvider = NotifierProvider<SearchQueryNotifier, String>(
  SearchQueryNotifier.new,
);

class SearchQueryNotifier extends Notifier<String> {
  @override
  String build() => '';

  void set(String value) => state = value;
}
