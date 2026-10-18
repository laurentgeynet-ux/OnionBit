// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Hôte Win32 du navigateur embarqué — adoption du handle natif
/// publié par `onionbit-browser --host-shell` (`DILLO_HOST_HANDLE=`,
/// rendu par `POST /api/browser/launch` quand `embed:true`).
///
/// Mécanisme : fenêtre **possédée** (owned top-level —
/// `GWLP_HWNDPARENT` sur le HWND du runner Flutter,
/// `FLUTTER_RUNNER_WIN32_WINDOW`) suivie par `SetWindowPos` vers
/// la zone de la page. Choisi plutôt que `SetParent`/`WS_CHILD`,
/// mesuré empiriquement : en `WS_CHILD`, chaque présentation de la
/// surface flip-model de `FLUTTERVIEW` peut couvrir le HWND enfant
/// une frame sur deux (« airspace ») → ~50 % des frames montrent la
/// page de l'app à la place du navigateur (flicker massif au
/// survol) ; en top-level possédée, plus aucune frame noire.
/// La fenêtre possédée reste un top-level normal : Windows la garde
/// au-dessus du propriétaire / la minimise avec lui automatiquement.
/// Aucun FFI vers dillo, aucun canal de pixels — le navigateur garde
/// sa boucle native et son processus (confinement identique :
/// profil éphémère, proxy loopback).
///
/// Windows uniquement : l'appelant garde `Platform.isWindows` ;
/// Linux/X11 (`XReparentWindow` sur l'XID) est un chantier séparé,
/// Wayland ne permet pas le reparenting inter-clients.
library;

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';
import 'package:win32/win32.dart';

/// Adoption et suivi d'un HWND navigateur dans la fenêtre Flutter.
class BrowserEmbedHost {
  /// HWND enfant courant (0 = aucun). Posé dès réception du handle,
  /// avant l'adoption — sert d'état « navigateur lancé » à l'UI.
  int childHwnd = 0;

  int _savedStyle = 0;
  int _savedExStyle = 0;
  int _hostHwnd = 0;
  bool _adopted = false;

  /// Dernier rect **écran** appliqué à la fenêtre — évite de
  /// rejouer `SetWindowPos` au rythme du poll quand rien n'a
  /// bougé. Écran (pas client) : déplacer la fenêtre hôte doit
  /// re-sync la fenêtre trackée même si le rect local est stable.
  int _lastL = -1, _lastT = -1, _lastW = -1, _lastH = -1;

  /// Fenêtre adoptée et suivie.
  bool get embedded => _adopted;

  /// Fenêtre **possédée** (owned top-level) — pas `WS_CHILD` :
  /// reparentée dans la surface D3D de Flutter, l'enfant est
  /// recouvert une frame sur deux par le swapchain flip-model à
  /// chaque présentation → flicker massif mesuré (~50 % de frames
  /// noires au survol, reproductible). Une fenêtre possédée reste
  /// un top-level normal — Windows la garde au-dessus du
  /// propriétaire et la minimise avec lui (≙ dialogue),
  /// `WS_EX_TOOLWINDOW` la retire de la taskbar, `WS_EX_COMPOSITED`
  /// double-buffe son subtree (ceinture-et-bretelles anti-tearing
  /// GDI, même si le blit `GetDC` de softbuffer le contourne).
  /// `false` → le navigateur reste en fenêtre externe.
  bool adopt(int hwnd) {
    if (_adopted || hwnd == 0 || !Platform.isWindows) return _adopted;
    childHwnd = hwnd;
    final host = _findHostHwnd();
    if (host == null) return false;
    _hostHwnd = host.address;
    final child = HWND(Pointer.fromAddress(hwnd));
    _savedStyle = GetWindowLongPtr(child, GWL_STYLE).value;
    _savedExStyle = GetWindowLongPtr(child, GWL_EXSTYLE).value;
    SetWindowLongPtr(child, GWLP_HWNDPARENT, host.address);
    SetWindowLongPtr(
      child,
      GWL_STYLE,
      (_savedStyle &
              ~(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_CHILD)) |
          WS_POPUP |
          WS_VISIBLE,
    );
    SetWindowLongPtr(
      child,
      GWL_EXSTYLE,
      (_savedExStyle & ~WS_EX_APPWINDOW) |
          WS_EX_TOOLWINDOW |
          WS_EX_COMPOSITED,
    );
    SetWindowPos(
      child,
      null,
      0,
      0,
      0,
      0,
      SET_WINDOW_POS_FLAGS(
        SWP_NOSIZE |
            SWP_NOMOVE |
            SWP_NOZORDER |
            SWP_NOACTIVATE |
            SWP_FRAMECHANGED,
      ),
    );
    ShowWindow(child, SW_SHOW);
    _adopted = true;
    return true;
  }

  /// Détache la fenêtre : propriétaire retiré + restauration des
  /// styles d'origine — elle redevient une fenêtre externe
  /// normale. Idempotent ; appelé au `dispose` de la page (changer
  /// d'onglet « sort » le navigateur sans le tuer).
  void release() {
    if (!_adopted || !Platform.isWindows) return;
    _adopted = false;
    final child = HWND(Pointer.fromAddress(childHwnd));
    SetWindowLongPtr(child, GWLP_HWNDPARENT, 0);
    SetWindowLongPtr(child, GWL_STYLE, _savedStyle);
    SetWindowLongPtr(child, GWL_EXSTYLE, _savedExStyle);
    SetWindowPos(
      child,
      null,
      0,
      0,
      0,
      0,
      SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE |
            SWP_NOSIZE |
            SWP_NOZORDER |
            SWP_NOACTIVATE |
            SWP_FRAMECHANGED,
      ),
    );
    ShowWindow(child, SW_SHOW);
    childHwnd = 0;
    _hostHwnd = 0;
    // Rect cache réinitialisé — une ré-adoption au même rect ne
    // doit pas être skippée (l'enfant serait laissé à l'ancienne
    // position).
    _lastL = _lastT = _lastW = _lastH = -1;
  }

  /// La fenêtre trackée est morte sans passer par `release`
  /// (croix de l'app, Ctrl+Q du navigateur, crash) — rien à
  /// restaurer : on oublie styles, hôte, cache de rect et hwnd.
  /// Sans ça l'état périmé court-circuite la ré-adoption suivante
  /// (`_adopted` → `adopt` early-return, cache rect → `syncTo`
  /// skippé) et le navigateur ré-ouvert reste en externe décorée.
  void forget() {
    _adopted = false;
    _hostHwnd = 0;
    _savedStyle = 0;
    _savedExStyle = 0;
    _lastL = _lastT = _lastW = _lastH = -1;
    childHwnd = 0;
  }

  /// Masque la fenêtre adoptée sans la détacher (`SW_HIDE`) —
  /// la page disparaît (changement d'onglet) mais la session du
  /// navigateur survit ; `reveal` la remet en place au retour sans
  /// relancer le processus.
  void hide() {
    if (!_adopted || !Platform.isWindows || childHwnd == 0) return;
    ShowWindow(HWND(Pointer.fromAddress(childHwnd)), SW_HIDE);
  }

  /// Réaffiche une fenêtre masquée par `hide` — appeler `syncTo`
  /// AVANT pour qu'elle réapparaisse directement sur la nouvelle
  /// zone (sinon un frame à l'ancienne position est visible).
  void reveal() {
    if (!_adopted || !Platform.isWindows || childHwnd == 0) return;
    ShowWindow(HWND(Pointer.fromAddress(childHwnd)), SW_SHOW);
  }

  /// Envoie `WM_CLOSE` — fermeture propre (≙ clic sur la croix,
  /// absente en mode non décoré). Le poll `alive` constate la mort.
  void close() {
    if (childHwnd == 0 || !Platform.isWindows) return;
    PostMessage(
      HWND(Pointer.fromAddress(childHwnd)),
      WM_CLOSE,
      WPARAM(0),
      LPARAM(0),
    );
  }

  /// `IsWindow` — faux dès que le processus navigateur est mort
  /// (croix, Ctrl+Q, crash) : l'UI retombe sur le tableau de bord.
  bool alive() =>
      childHwnd != 0 &&
      Platform.isWindows &&
      IsWindow(HWND(Pointer.fromAddress(childHwnd)));

  /// Repositionne l'enfant sur le rect de la zone embed (coords
  /// client de la fenêtre hôte = pixels physiques du FlutterView).
  /// No-op si le rect est inchangé. `SetWindowPos` émet `WM_SIZE` →
  /// winit `Resized` → le navigateur relayout et repeint sa surface
  /// lui-même (fix `layout_win` — pas de `RedrawWindow` forcé).
  void syncTo(int left, int top, int width, int height) {
    if (!_adopted || !Platform.isWindows || _hostHwnd == 0) return;
    // Top-level : position en coords **écran** — `left/top` arrivent
    // en coords client de la fenêtre hôte, `ClientToScreen` convertit.
    // Le dedup sur le rect écran rend la fenêtre suiveuse d'un
    // déplacement de l'app (le rect client ne change pas en drag).
    final pt = calloc<POINT>()
      ..ref.x = left
      ..ref.y = top;
    ClientToScreen(HWND(Pointer.fromAddress(_hostHwnd)), pt);
    final (sx, sy) = (pt.ref.x, pt.ref.y);
    free(pt);
    if (sx == _lastL && sy == _lastT && width == _lastW && height == _lastH) {
      return;
    }
    _lastL = sx;
    _lastT = sy;
    _lastW = width;
    _lastH = height;
    final child = HWND(Pointer.fromAddress(childHwnd));
    SetWindowPos(
      child,
      null,
      sx,
      sy,
      width,
      height,
      SET_WINDOW_POS_FLAGS(SWP_NOZORDER | SWP_NOACTIVATE),
    );
  }

  /// HWND de **notre** fenêtre runner Flutter — `FindWindow` seul
  /// prend la première `FLUTTER_RUNNER_WIN32_WINDOW` trouvée, qui
  /// peut appartenir à **une autre application Flutter** : on
  /// énumère les fenêtres de la classe et on filtre par PID
  /// (`GetWindowThreadProcessId`), sinon le navigateur serait
  /// reparenté dans la fenêtre d'un tiers.
  HWND? _findHostHwnd() {
    final className = 'FLUTTER_RUNNER_WIN32_WINDOW'.toNativeUtf16();
    final pidBuf = calloc<DWORD>();
    try {
      var prev = HWND(Pointer.fromAddress(0));
      while (true) {
        final h = FindWindowEx(
          HWND(Pointer.fromAddress(0)),
          prev,
          PCWSTR(className),
          null,
        ).value;
        if (h.address == 0) return null;
        pidBuf.value = 0;
        GetWindowThreadProcessId(h, pidBuf);
        if (pidBuf.value == pid) return h;
        prev = h;
      }
    } finally {
      free(className);
      free(pidBuf);
    }
  }
}
