import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'app_config.dart';
import 'daemon_api_resolver.dart';

/// Connexion au daemon persistée (`shared_preferences`) — URL de base
/// de `tribler-api` + clé éventuelle. Modifiable dans Réglages, prend
/// effet immédiatement (les providers `apiClient`/`sseClient`
/// surveillent cette source).
///
/// Ordre de résolution (miroir de la session injectée d'eMule-Rust) :
/// 1. réglage utilisateur pointant hors loopback (daemon distant) —
///    toujours respecté tel quel ;
/// 2. `configuration.json` du daemon local découvert automatiquement
///    (`daemon_api_resolver`) — source de vérité vivante : clé régénérée
///    et port aléatoire `http_port_running` se résolvent seuls ;
/// 3. préférences persistées puis défauts (`127.0.0.1:8085`, sans clé).
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
    final savedUrl = (prefs.getString(_kKeyBaseUrl) ?? '').trim();
    final savedKey = prefs.getString(_kKeyApiKey) ?? '';

    // 1. Daemon distant explicite : ne jamais le remplacer par la
    //    découverte locale.
    if (savedUrl.isNotEmpty && !_isLoopback(savedUrl)) {
      return AppConfig(baseUrl: savedUrl, apiKey: savedKey);
    }

    // 2. Daemon local : `configuration.json` (clé + port réel) prime sur
    //    les préférences — auto-cicatrisant si la clé est régénérée ou
    //    le port relancé en aléatoire.
    final discovered = resolveDaemonApi();
    if (discovered != null) return discovered;

    // 3. Repli : préférences (loopback) puis défauts.
    return AppConfig(
      baseUrl: savedUrl.isNotEmpty ? savedUrl : const AppConfig().baseUrl,
      apiKey: savedKey,
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

/// `true` si l'URL pointe vers le daemon local (loopback) : la
/// découverte via `configuration.json` s'y applique.
bool _isLoopback(String url) {
  final host = Uri.tryParse(url)?.host ?? '';
  return host == '127.0.0.1' ||
      host == 'localhost' ||
      host == '::1' ||
      host == '[::1]';
}
