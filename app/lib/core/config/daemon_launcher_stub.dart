// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'app_config.dart';

/// Stub non-desktop (web) : pas de processus local lançable — la
/// connexion vient des réglages utilisateur persistés.
Future<AppConfig?> ensureDaemonRunning() async => null;
