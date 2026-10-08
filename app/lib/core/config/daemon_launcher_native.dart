// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Lancement du daemon local depuis l'UI (`dart:io`, desktop).
///
/// « Daemon enfant lancé par l'app » (décision V1) : `OnionBit.exe`
/// démarre `onionbit-daemon.exe` si l'API ne répond pas ; si le daemon
/// tourne déjà (lancé à la main, resté en systray, ou autostart) l'UI
/// s'y connecte directement — aucun script de lancement requis.
library;

import 'dart:async';
import 'dart:io';

import 'app_config.dart';
import 'daemon_api_resolver_native.dart' as resolver;
import 'ui_log.dart';

/// Durée max d'attente du démarrage du daemon (premier run : migration
/// SQLite + génération de la clé API + bind — même fenêtre que
/// l'ancien lanceur `demarrer.ps1`).
const _kStartupTimeout = Duration(seconds: 30);
const _kPollInterval = Duration(milliseconds: 500);
/// Timeout d'une sonde HTTP — un daemon vivant répond immédiatement
/// en loopback.
const _kProbeTimeout = Duration(milliseconds: 800);

/// Garantit un daemon local vivant et renvoie sa config de connexion.
///
/// 1. `configuration.json` résolu + API vivante → renvoyé tel quel ;
/// 2. API morte → `onionbit-daemon[.exe]` voisin de l'exe lancé détaché
///    (`--state-dir <exe>/state`), API sondée jusqu'à timeout — port
///    réel et clé relus à chaque tentative (`http_port=0` possible) ;
/// 3. binaire absent (`flutter run`, install partielle) → `null`
///    (connexion manuelle dans « Connexion daemon »).
///
/// `ONIONBIT_API_KEY` dans l'environnement = setup externe piloté : on
/// ne lance jamais de daemon enfant. `ONIONBIT_DAEMON_EXE` surcharge le
/// chemin du binaire (boucle de développement).
Future<AppConfig?> ensureDaemonRunning() async {
  final t0 = DateTime.now();
  var config = resolver.resolveDaemonApi();
  uiLog('resolve -> ${config?.baseUrl ?? "null"}');
  if (config != null && await isDaemonApiAlive(config)) {
    uiLog('alive en ${DateTime.now().difference(t0).inMilliseconds} ms');
    return config;
  }
  if ((Platform.environment['ONIONBIT_API_KEY'] ?? '').trim().isNotEmpty) {
    return config;
  }

  final exe = _daemonExe();
  if (exe == null) {
    uiLog('daemon exe introuvable');
    return null;
  }
  final exeDir = exe.parent;
  // ADR-0018 : dans un bundle portable (`OnionBit.portable` à un
  // ancêtre de l'exe, layout `<root>/<os>/…`), le daemon résout
  // lui-même `<root>/state` — un `--state-dir <exe>/state` viserait
  // `<root>/<os>/state` à tort. Hors bundle : la convention
  // `<exe>/state` historique est conservée.
  final portable = _portableRoot(exeDir);
  final args = portable != null
      ? const ['--first-run-gate']
      : [
          '--state-dir',
          '${exeDir.path}${Platform.pathSeparator}state',
          '--first-run-gate',
        ];

  try {
    // Détaché : le daemon survit à la fermeture de l'UI (il vit dans
    // sa propre icône systray depuis l'étape 29). `--first-run-gate`
    // (ADR-0016) : un state_dir vierge reste en `identity_pending`
    // — aucune clé jetable n'est créée avant le choix utilisateur.
    uiLog('spawn ${exe.path} ${args.join(' ')}');
    await Process.start(
      exe.path,
      args,
      mode: ProcessStartMode.detached,
      workingDirectory: exeDir.path,
    );
  } on ProcessException {
    uiLog('Process.start echoue');
    return null;
  }

  final deadline = DateTime.now().add(_kStartupTimeout);
  while (DateTime.now().isBefore(deadline)) {
    await Future<void>.delayed(_kPollInterval);
    config = resolver.resolveDaemonApi();
    if (config != null && await isDaemonApiAlive(config)) {
      uiLog(
        'alive sur ${config.baseUrl} apres spawn '
        '+${DateTime.now().difference(t0).inMilliseconds} ms',
      );
      return config;
    }
  }
  uiLog('timeout ${_kStartupTimeout.inSeconds}s sans reponse du daemon');
  return null;
}

/// Racine du bundle portable si `OnionBit.portable` marque un ancêtre
/// de `dir` (miroir de `paths::find_portable_root` du daemon —
/// ADR-0018). `null` hors bundle portable.
Directory? _portableRoot(Directory dir) {
  var d = dir;
  while (true) {
    if (File('${d.path}${Platform.pathSeparator}OnionBit.portable')
        .existsSync()) {
      return d;
    }
    final parent = d.parent;
    if (parent.path == d.path) return null;
    d = parent;
  }
}

/// `onionbit-daemon[.exe]` voisin de l'exécutable de l'UI
/// (`ONIONBIT_DAEMON_EXE` en premier — ex. `target\debug\…` en dev).
File? _daemonExe() {
  final override = Platform.environment['ONIONBIT_DAEMON_EXE']?.trim();
  if (override != null && override.isNotEmpty) {
    final f = File(override);
    if (f.existsSync()) return f;
  }
  final exeDir = File(Platform.resolvedExecutable).parent;
  final name = Platform.isWindows ? 'onionbit-daemon.exe' : 'onionbit-daemon';
  final f = File('${exeDir.path}${Platform.pathSeparator}$name');
  return f.existsSync() ? f : null;
}

/// Toute réponse HTTP — y compris 401 sans clé — prouve que l'API est
/// en vie (même logique que le `Test-ApiAlive` de l'ancien
/// `demarrer.ps1`).
/// Publique pour les tests (`daemon_launcher_test.dart`).
Future<bool> isDaemonApiAlive(AppConfig config) async {
  final client = HttpClient()..connectionTimeout = _kProbeTimeout;
  try {
    final request = await client
        .getUrl(Uri.parse('${config.baseUrl}/api/events/info'))
        .timeout(_kProbeTimeout);
    if (config.apiKey.isNotEmpty) {
      request.headers.set('X-Api-Key', config.apiKey);
    }
    final response = await request.close().timeout(_kProbeTimeout);
    await response.drain<void>();
    return true;
  } catch (_) {
    return false;
  } finally {
    client.close();
  }
}
