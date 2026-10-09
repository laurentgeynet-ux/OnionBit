// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Capacités plateforme de l'appairage — import conditionnel
/// (`dart:io` n'existe pas sur web ; le stub rend `false` partout).
library;

export 'scan_support_stub.dart'
    if (dart.library.io) 'scan_support_io.dart';
