// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : client `package:http` standard — sockets
/// réelles, réponses streamées natives.
library;

import 'package:http/http.dart' as http;

http.Client createHttpClient() => http.Client();
