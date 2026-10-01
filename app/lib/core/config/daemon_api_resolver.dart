/// Résolution automatique de la connexion au daemon local, spécifique
/// plateforme — même mécanisme que `session_resolver` d'eMule-Rust :
/// `onionbit-daemon` persiste `state_dir/configuration.json`
/// (`api.key`, `api.http_port_running`) et l'UI le relit au démarrage,
/// comme la GUI Tribler lit `api/key` + `api/http_port_running`.
///
/// Sur desktop (`dart.library.io`) : variable d'environnement
/// `ONIONBIT_API_KEY` (+ `ONIONBIT_API` pour l'URL), puis
/// `configuration.json` cherché dans les répertoires candidats (bundle
/// `dist\`, boucle de dev). Sur web/stub : pas de fichier local lisible
/// → `null` (réglages manuels dans « Connexion daemon »).
library;

import 'app_config.dart';
import 'daemon_api_resolver_stub.dart'
    if (dart.library.io) 'daemon_api_resolver_native.dart' as impl;

/// `AppConfig` découverte depuis le daemon local, ou `null` si aucune
/// source n'est disponible (web, daemon jamais lancé, fichier absent).
AppConfig? resolveDaemonApi() => impl.resolveDaemonApi();
