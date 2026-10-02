// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : URL dans le navigateur par défaut du système.
library;

import 'dart:io';

import 'package:flutter/foundation.dart';

import '../config/ui_log.dart';

void openExternalUrl(String url) {
  try {
    if (Platform.isWindows) {
      // `start` exige un titre vide avant l'URL.
      Process.run('cmd', ['/c', 'start', '', url]);
    } else if (Platform.isMacOS) {
      Process.run('open', [url]);
    } else if (Platform.isLinux) {
      Process.run('xdg-open', [url]);
    }
  } on Object catch (e) {
    uiLog('ouverture URL externe en échec : $e');
    debugPrint('openExternalUrl($url) : $e');
  }
}
