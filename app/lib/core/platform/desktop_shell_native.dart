/// Init desktop (Windows/Linux/macOS) : fenêtre (taille minimale,
/// titre). Isolé derrière un import conditionnel — `window_manager`
/// dépend de `dart:ffi`, non compilable pour le web.
library;

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

/// Fenêtre utilisable au plus petit en 960×560 (sidebar + table).
const Size _kMinWindowSize = Size(960, 560);

Future<void> initDesktopShell() async {
  if (!(Platform.isWindows || Platform.isLinux || Platform.isMacOS)) {
    return;
  }
  await windowManager.ensureInitialized();
  const options = WindowOptions(
    minimumSize: _kMinWindowSize,
    title: 'Tribler-Rust',
    titleBarStyle: TitleBarStyle.normal,
  );
  await windowManager.waitUntilReadyToShow(options, () async {
    await windowManager.show();
    await windowManager.focus();
  });
}
