/// Résolution desktop (`dart:io`) de la connexion au daemon local.
///
/// Ordre de résolution (miroir de `session_resolver_native.dart`
/// d'eMule-Rust) :
/// 1. `TRIBLER_API_KEY` dans l'environnement du processus — injectée par
///    un lanceur ou un service wrapper ; `TRIBLER_API` surcharge l'URL ;
/// 2. `configuration.json` écrit par `tribler-daemon` dans son
///    `state_dir`, cherché dans les répertoires candidats.
///
/// `configuration.json` expose `api.key` (clé `X-Api-Key`) et le port
/// réellement lié `api.http_port_running` (prioritaire sur le port
/// demandé `api.http_port`, qui peut valoir 0 = aléatoire).
library;

import 'dart:convert';
import 'dart:io';

import 'app_config.dart';

/// Répertoires candidats contenant `configuration.json`, par priorité :
/// `<exe>/state` et `<exe>/../state` (bundle `dist\` : `tribler_ui.exe`
/// à la racine, daemon lancé avec `--state-dir <dist>\state`),
/// `<cwd>/.tribler`, `<cwd>/../.tribler` et `<cwd>/state` (boucle de
/// développement : `flutter run` depuis `app\`, daemon depuis la racine
/// du dépôt ou son propre `--state-dir`).
List<Directory> _candidateStateDirs() {
  final exeDir = File(Platform.resolvedExecutable).parent;
  final cwd = Directory.current;
  final sep = Platform.pathSeparator;
  return [
    Directory('${exeDir.path}${sep}state'),
    Directory('${exeDir.parent.path}${sep}state'),
    Directory('${cwd.path}$sep.tribler'),
    Directory('${cwd.parent.path}$sep.tribler'),
    Directory('${cwd.path}${sep}state'),
  ];
}

/// Lit `configuration.json` du premier répertoire candidat existant.
/// Renvoie `null` si le fichier est absent, illisible ou sans clé API
/// exploitable (daemon pas encore démarré une première fois).
AppConfig? _fromConfigFile() {
  for (final dir in _candidateStateDirs()) {
    final file =
        File('${dir.path}${Platform.pathSeparator}configuration.json');
    if (!file.existsSync()) continue;
    try {
      final root = jsonDecode(file.readAsStringSync());
      if (root is! Map) return null;
      final api = root['api'];
      if (api is! Map) return null;
      final key = api['key'];
      if (key is! String || key.isEmpty) return null;

      int portOf(String name) {
        final v = api[name];
        return v is int ? v : 0;
      }

      final port = portOf('http_port_running') > 0
          ? portOf('http_port_running')
          : portOf('http_port');
      final host = api['http_host'];
      return AppConfig(
        baseUrl: port > 0
            ? 'http://${host is String && host.isNotEmpty ? host : '127.0.0.1'}:$port'
            : const AppConfig().baseUrl,
        apiKey: key,
      );
    } on FormatException {
      return null;
    } on FileSystemException {
      return null;
    }
  }
  return null;
}

/// Session résolue depuis l'environnement ou `configuration.json` du
/// daemon. Renvoie `null` si aucune source n'est disponible — l'app
/// retombe alors sur les préférences persistées puis les défauts.
AppConfig? resolveDaemonApi() {
  final env = Platform.environment;
  final envKey = env['TRIBLER_API_KEY']?.trim() ?? '';
  if (envKey.isNotEmpty) {
    final envUrl = env['TRIBLER_API']?.trim() ?? '';
    return AppConfig(
      baseUrl: envUrl.isNotEmpty ? envUrl : const AppConfig().baseUrl,
      apiKey: envKey,
    );
  }
  return _fromConfigFile();
}
