// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Fabrique de `http.Client` spécifique plateforme.
///
/// Sur desktop (`dart.library.io`) : `http.Client()` standard (sockets
/// réels, corps en flux). Sur web : `FetchClient` — le `BrowserClient`
/// XHR de `package:http` bufferise la réponse entière, ce qui rend
/// impossibles le SSE `/api/events` et le speed test de circuit
/// (`getStreamedLines`) ; l'API Fetch livre le corps en flux continu.
library;

import 'package:http/http.dart' as http;

import 'http_transport_web.dart'
    if (dart.library.io) 'http_transport_native.dart' as impl;

/// Client HTTP de la plateforme (réponses streamées incluses).
http.Client createHttpClient() => impl.createHttpClient();
