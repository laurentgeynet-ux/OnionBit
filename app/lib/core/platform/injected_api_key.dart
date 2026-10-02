// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Clé API injectée par le daemon dans l'`index.html` servi (meta
/// `onionbit-api-key`, `api/web_ui_inject_key`) — équivalent web de la
/// lecture de `configuration.json` par l'UI desktop : la page servie
/// en same-origin loopback se connecte sans saisie.
///
/// Spécifique plateforme : DOM navigateur sur web, `null` ailleurs
/// (le desktop passe par `daemon_api_resolver`).
library;

import 'injected_api_key_web.dart'
    if (dart.library.io) 'injected_api_key_native.dart' as impl;

/// Clé lue dans `<meta name="onionbit-api-key">`, ou `null` (native,
/// meta absente ou injection désactivée côté daemon).
String? injectedApiKey() => impl.injectedApiKey();
