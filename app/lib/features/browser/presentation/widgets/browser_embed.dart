// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Façade du reparenting du navigateur embarqué — aiguillage
/// plateforme (même convention que `core/platform/desktop_shell.dart`) :
///
/// - `dart:io` disponible → implémentation Win32
///   (`browser_embed_native.dart`, `dart:ffi`/`win32`) ;
/// - web → stub no-op (`browser_embed_stub.dart`) — `dart:ffi`
///   n'existe pas sur cette cible, un import direct de l'implé
///   casserait `flutter build web`.
library;

export 'browser_embed_stub.dart'
    if (dart.library.io) 'browser_embed_native.dart';
