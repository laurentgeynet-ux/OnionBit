/// Garantit qu'un daemon local répond avant la résolution de la
/// connexion — décision V1 de `flutter_architecture.md` : « daemon
/// enfant lancé par l'app », avec connexion directe si un daemon
/// tourne déjà.
///
/// Sur desktop (`dart.library.io`) : si l'API découverte ne répond
/// pas, `onionbit-daemon[.exe]` à côté de l'exécutable de l'UI est
/// lancé détaché (`--state-dir <exe>/state`, la disposition du bundle
/// `dist\`), puis sondé ~30 s. Sur web : no-op (`null` — un
/// navigateur ne peut pas lancer de processus).
library;

import 'app_config.dart';
import 'daemon_launcher_stub.dart'
    if (dart.library.io) 'daemon_launcher_native.dart' as impl;

/// Renvoie l'`AppConfig` d'un daemon local vivant (déjà présent ou
/// fraîchement lancé), `null` si aucun n'est joignable — l'app retombe
/// alors sur les préférences persistées puis les défauts.
Future<AppConfig?> ensureDaemonRunning() => impl.ensureDaemonRunning();
