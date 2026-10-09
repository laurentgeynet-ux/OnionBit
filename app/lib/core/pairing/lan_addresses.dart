// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Adresses IPv4 LAN du poste — import conditionnel : `dart:io` n'est
/// pas compilable sur web, le stub rend une liste vide.
library;

export 'lan_addresses_stub.dart'
    if (dart.library.io) 'lan_addresses_io.dart';
