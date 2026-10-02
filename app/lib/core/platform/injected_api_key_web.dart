// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Lecture web de la clé API injectée par le daemon dans
/// `index.html` (`<meta name="onionbit-api-key" content="…">` —
/// `webui.rs`, `api/web_ui_inject_key`). `null` si absente.
library;

import 'package:web/web.dart' as web;

String? injectedApiKey() {
  final el = web.document
      .querySelector('meta[name="onionbit-api-key"]');
  final content = el?.getAttribute('content')?.trim();
  return (content == null || content.isEmpty) ? null : content;
}
