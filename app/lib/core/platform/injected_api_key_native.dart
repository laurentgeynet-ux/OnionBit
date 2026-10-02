// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Stub natif (`dart:io`) : pas de DOM — la clé vient de
/// `configuration.json` via `daemon_api_resolver_native`.
String? injectedApiKey() => null;
