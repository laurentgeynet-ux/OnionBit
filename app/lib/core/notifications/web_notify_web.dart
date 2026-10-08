// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : notification via l'API `Notification` du navigateur.
///
/// La permission est demandée à la première émission ; si elle est
/// refusée ou indisponible, l'app reste silencieuse (le snackbar
/// in-app couvre déjà l'événement).
library;

import 'dart:js_interop';

import 'package:web/web.dart' as web;

void notifySystem(String title, String body) {
  try {
    switch (web.Notification.permission) {
      case 'granted':
        web.Notification(
          title,
          web.NotificationOptions(body: body, tag: 'onionbit'),
        );
      case 'default':
        web.Notification.requestPermission().toDart.then((perm) {
          if (perm.toDart == 'granted') {
            web.Notification(
              title,
              web.NotificationOptions(body: body, tag: 'onionbit'),
            );
          }
        });
      default:
        break; // 'denied' : ne pas re-demander.
    }
  } on Object {
    // Notification API absente (contexte non sécurisé) : silencieux.
  }
}
