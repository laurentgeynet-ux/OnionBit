// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : URL dans un nouvel onglet (`window.open`).
library;

import 'package:web/web.dart' as web;

void openExternalUrl(String url) {
  web.window.open(url, '_blank');
}
