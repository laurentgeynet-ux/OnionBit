import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../data/rest_settings_repository.dart';
import '../../domain/settings_repository.dart';

final settingsRepositoryProvider = Provider<SettingsRepository>(
  (ref) => RestSettingsRepository(ref.watch(apiClientProvider)),
);

final daemonSettingsProvider =
    FutureProvider.autoDispose<Map<String, dynamic>>(
      (ref) => ref.watch(settingsRepositoryProvider).get(),
    );

/// Espace disque d'un répertoire (`null` = dossier de téléchargement
/// par défaut) — `{total, used, free}`.
final dirSpaceProvider = FutureProvider.autoDispose
    .family<Map<String, int>, String?>(
      (ref, dir) => ref
          .watch(settingsRepositoryProvider)
          .dirSpace(directory: dir),
    );

/// Items découverts par les flux RSS (`GET /api/rss`).
final rssItemsProvider =
    FutureProvider.autoDispose<List<Map<String, dynamic>>>(
      (ref) => ref.watch(settingsRepositoryProvider).rssItems(),
    );

/// Versions connues du daemon (`GET /api/versioning/versions`).
final versionsProvider = FutureProvider.autoDispose<Map<String, dynamic>>(
  (ref) => ref.watch(settingsRepositoryProvider).versions(),
);
