// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'app_config.dart';

/// Stub non-desktop (web) : pas de fichier de configuration local
/// lisible — la connexion vient des réglages utilisateur persistés.
AppConfig? resolveDaemonApi() => null;
