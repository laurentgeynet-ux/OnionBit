// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Journal UI desktop (`dart:io`) : `<exe>/state/logs/ui.log` —
/// à côté du `onionbit.log` du daemon. Tolère toute erreur d'écriture.
library;

import 'dart:io';

File? _logFile() {
  try {
    final exeDir = File(Platform.resolvedExecutable).parent;
    final sep = Platform.pathSeparator;
    for (final rel in ['state', '..${sep}state']) {
      final dir = Directory('${exeDir.path}$sep$rel');
      if (dir.existsSync()) {
        return File('${dir.path}${sep}logs${sep}ui.log');
      }
    }
  } catch (_) {}
  return null;
}

void uiLog(String msg) {
  try {
    _logFile()
      ?..createSync(recursive: true)
      ..writeAsStringSync(
        '${DateTime.now().toIso8601String()}  $msg\n',
        mode: FileMode.append,
      );
  } catch (_) {}
}

Future<String> readUiLog() async {
  try {
    final exeDir = File(Platform.resolvedExecutable).parent;
    final sep = Platform.pathSeparator;
    for (final dir in [
      Directory('${exeDir.path}${sep}state'),
      Directory('${exeDir.parent.path}${sep}state'),
    ]) {
      // `ui_connect.log` = ancien nom du journal (même contenu).
      for (final name in const ['ui.log', 'ui_connect.log']) {
        final f = File('${dir.path}${sep}logs$sep$name');
        if (f.existsSync()) {
          final lines = f.readAsLinesSync();
          return lines.length > 300
              ? lines.sublist(lines.length - 300).join('\n')
              : lines.join('\n');
        }
      }
    }
  } catch (_) {}
  return '';
}
