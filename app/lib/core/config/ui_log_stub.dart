// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Stub hors desktop : pas de fichier de journal UI.
void uiLog(String msg) {}

Future<String> readUiLog() async => '';
