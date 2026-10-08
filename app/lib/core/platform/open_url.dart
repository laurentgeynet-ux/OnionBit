// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Ouverture d'une URL hors de l'app, spécifique plateforme.
///
/// Desktop : navigateur par défaut (`start`/`open`/`xdg-open`).
/// Web : nouvel onglet (`window.open`) — le streaming
/// `/api/downloads/{ih}/stream/{i}` s'y lit via le player HTML5.
library;

import 'open_url_web.dart'
    if (dart.library.io) 'open_url_native.dart' as impl;

/// Ouvre `url` dans le navigateur par défaut / un nouvel onglet.
void openExternalUrl(String url) => impl.openExternalUrl(url);
