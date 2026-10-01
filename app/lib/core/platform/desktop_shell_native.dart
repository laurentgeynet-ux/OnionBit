// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Init desktop (Windows/Linux/macOS) : fenêtre (taille minimale,
/// titre). Isolé derrière un import conditionnel — `window_manager`
/// dépend de `dart:ffi`, non compilable pour le web.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import '../config/ui_log.dart';

/// Fenêtre utilisable au plus petit en 960×560 (sidebar + table).
const Size _kMinWindowSize = Size(960, 560);

Future<void> initDesktopShell() async {
  if (!(Platform.isWindows || Platform.isLinux || Platform.isMacOS)) {
    return;
  }
  if (Platform.isWindows && kReleaseMode) {
    // Association `.torrent` → onionbit_ui (HKCU, sans droits
    // admin) : double-clic / « Ouvrir avec » lance l'app avec le
    // fichier en argv.
    unawaited(_registerTorrentFileAssoc());
  }
  await windowManager.ensureInitialized();
  const options = WindowOptions(
    minimumSize: _kMinWindowSize,
    title: 'OnionBit',
    titleBarStyle: TitleBarStyle.normal,
  );
  await windowManager.waitUntilReadyToShow(options, () async {
    await windowManager.show();
    await windowManager.focus();
  });
}

/// Enregistre l'association `.torrent` → cet exécutable dans
/// HKCU (`reg add`, sans elevation). Idempotent : reecrit a
/// chaque demarrage pour suivre un deplacement de l'exe.
Future<void> _registerTorrentFileAssoc() async {
  final exe = Platform.resolvedExecutable;
  const progid = 'OnionBit.torrent';
  try {
    for (final args in [
      ['add', r'HKCU\Software\Classes\.torrent', '/ve', '/d', progid, '/f'],
      [
        'add',
        r'HKCU\Software\Classes\.torrent\OpenWithProgids',
        '/v',
        progid,
        '/t',
        'REG_SZ',
        '/d',
        '',
        '/f',
      ],
      [
        'add',
        'HKCU\\Software\\Classes\\$progid',
        '/ve',
        '/d',
        'OnionBit torrent',
        '/f',
      ],
      [
        'add',
        'HKCU\\Software\\Classes\\$progid\\DefaultIcon',
        '/ve',
        '/d',
        '"$exe",0',
        '/f',
      ],
      [
        'add',
        'HKCU\\Software\\Classes\\$progid\\shell\\open\\command',
        '/ve',
        '/d',
        '"$exe" "%1"',
        '/f',
      ],
    ]) {
      await Process.run('reg', args);
    }
    uiLog('association .torrent -> $exe');
  } catch (e) {
    uiLog('association .torrent en echec : $e');
  }
}

/// Ouvre un chemin de dossier ou fichier dans l'explorateur natif du système.
Future<void> openPath(String path) async {
  try {
    if (Platform.isWindows) {
      await Process.run('explorer.exe', [path]);
    } else if (Platform.isMacOS) {
      await Process.run('open', [path]);
    } else if (Platform.isLinux) {
      await Process.run('xdg-open', [path]);
    }
  } catch (_) {
    // Échec silencieux si le chemin n'existe pas ou shell indisponible
  }
}
