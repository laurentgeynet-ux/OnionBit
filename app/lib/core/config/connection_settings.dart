// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'app_config.dart';
import 'daemon_launcher.dart';
import 'ui_log.dart';

/// Connexion au daemon persistée (`shared_preferences`) — URL de base
/// de `onionbit-api` + clé éventuelle. Modifiable dans Réglages, prend
/// effet immédiatement (les providers `apiClient`/`sseClient`
/// surveillent cette source).
///
/// Ordre de résolution (miroir de la session injectée d'eMule-Rust) :
/// 1. réglage utilisateur pointant hors loopback (daemon distant) —
///    toujours respecté tel quel ;
/// 2. daemon local garanti vivant puis découvert (`daemon_launcher` :
///    lance `onionbit-daemon` s'il ne tourne pas, relit
///    `configuration.json` — clé régénérée et port aléatoire
///    `http_port_running` se résolvent seuls) ;
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

    // 2. Daemon local : lancé s'il ne tourne pas (binaire voisin de
    //    l'exe), puis `configuration.json` (clé + port réel) prime sur
    //    les préférences — auto-cicatrisant si la clé est régénérée ou
    //    le port relancé en aléatoire.
    final discovered = await ensureDaemonRunning();
    if (discovered != null) return discovered;

    // 3. Repli : préférences (loopback) puis défauts.
    return AppConfig(
      baseUrl: savedUrl.isNotEmpty ? savedUrl : const AppConfig().baseUrl,
      apiKey: savedKey,
    );
  }

  /// Re-résout la connexion locale (relecture de `configuration.json`,
  /// daemon relancé si mort). Appelé par le watchdog quand le SSE est
  /// coupé durablement : sans ça, l'app resterait figée sur le port
  /// `http_port_running` périmé lu au démarrage si le daemon a (re)bindé
  /// sur un autre port entre-temps.
  Future<void> rediscover() async {
    final current = state.value;
    if (current == null || !_isLoopback(current.baseUrl)) return;
    final discovered = await ensureDaemonRunning();
    if (discovered != null &&
        (discovered.baseUrl != current.baseUrl ||
            discovered.apiKey != current.apiKey)) {
      uiLog(
        'redecouverte : ${current.baseUrl} -> ${discovered.baseUrl}',
      );
      state = AsyncData(discovered);
    }
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
