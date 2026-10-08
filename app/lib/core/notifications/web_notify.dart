// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Notification système hors de l'app, spécifique plateforme.
///
/// Web : API `Notification` du navigateur (permission demandée à la
/// première émission — le daemon peut finir un torrent pendant que
/// l'onglet est en arrière-plan). Desktop : no-op (le snackbar in-app
/// suffit ; une notification OS dédiée n'existe pas encore).
library;

import 'web_notify_web.dart'
    if (dart.library.io) 'web_notify_native.dart' as impl;

/// Émet une notification navigateur (web) ; no-op ailleurs.
void notifySystem(String title, String body) => impl.notifySystem(title, body);
