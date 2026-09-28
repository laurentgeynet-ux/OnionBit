import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'app_config.dart';

/// Connexion au daemon persistée (`shared_preferences`) — URL de base
/// de `tribler-api` + clé éventuelle. Modifiable dans Réglages, prend
/// effet immédiatement (les providers `apiClient`/`sseClient`
/// surveillent cette source).
const _kKeyBaseUrl = 'api.baseUrl';
const _kKeyApiKey = 'api.key';

final connectionSettingsProvider =
    AsyncNotifierProvider<ConnectionSettingsNotifier, AppConfig>(
      ConnectionSettingsNotifier.new,
    );

class ConnectionSettingsNotifier extends AsyncNotifier<AppConfig> {
  @override
  Future<AppConfig> build() async {
    final prefs = await SharedPreferences.getInstance();
    return AppConfig(
      baseUrl: prefs.getString(_kKeyBaseUrl) ?? const AppConfig().baseUrl,
      apiKey: prefs.getString(_kKeyApiKey) ?? '',
    );
  }

  /// Persiste et applique une nouvelle configuration de connexion.
  Future<void> save({String? baseUrl, String? apiKey}) async {
    final current = state.value ?? const AppConfig();
    final next = AppConfig(
      baseUrl: (baseUrl ?? current.baseUrl).trim(),
      apiKey: apiKey ?? current.apiKey,
    );
    state = AsyncData(next);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kKeyBaseUrl, next.baseUrl);
    await prefs.setString(_kKeyApiKey, next.apiKey);
  }
}
