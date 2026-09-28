import 'app_config.dart';

/// Stub non-desktop (web) : pas de processus local lançable — la
/// connexion vient des réglages utilisateur persistés.
Future<AppConfig?> ensureDaemonRunning() async => null;
