// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Stub web/mobile de [BrowserEmbedHost] : pas de reparenting
/// Win32 possible hors desktop Windows — l'implémentation native
/// (`browser_embed_native.dart`) dépend de `dart:ffi`/`win32`,
/// non compilable pour le web. Toutes les opérations sont des
/// no-op ; `embedded`/`alive` restent faux — la page retombe sur
/// le tableau de bord (navigateur en fenêtre externe indisponible).
library;

/// No-op hors desktop Windows — mêmes signatures que l'implé
/// native pour que `browser_page.dart` compile partout.
class BrowserEmbedHost {
  /// HWND enfant courant (0 = aucun).
  int childHwnd = 0;

  /// Jamais adopté hors Windows natif.
  bool get embedded => false;

  /// Adoption impossible hors Windows natif.
  bool adopt(int hwnd) => false;

  void release() {}

  void forget() {}

  void hide() {}

  void reveal() {}

  void close() {}

  /// Pas de fenêtre trackée hors Windows natif.
  bool alive() => false;

  void syncTo(int left, int top, int width, int height) {}
}
