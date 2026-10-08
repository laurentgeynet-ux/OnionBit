// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : `FetchClient` (Fetch API) — seul transport navigateur qui
/// livre le corps en flux continu (`ReadableStream`). `BrowserClient`
/// (XHR) attend la fin de la réponse : un flux SSE n'émettrait jamais.
library;

import 'package:fetch_client/fetch_client.dart';
import 'package:http/http.dart' as http;

http.Client createHttpClient() => FetchClient(mode: RequestMode.cors);
