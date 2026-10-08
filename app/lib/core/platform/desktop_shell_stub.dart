// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Stub web : pas de window manager — l'init desktop est un no-op.
library;

Future<void> initDesktopShell() async {}

Future<void> openPath(String path) async {}
