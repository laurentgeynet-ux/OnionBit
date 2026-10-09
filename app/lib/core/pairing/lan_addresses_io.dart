// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:io';

/// IPv4 non-loopback des interfaces du poste (propositions pour le QR
/// d'appairage quand `api.http_host` est en loopback). Liste vide si
/// l'énumération échoue — le champ hôte reste en saisie libre.
Future<List<String>> lanIpv4Addresses() async {
  try {
    final ifaces = await NetworkInterface.list(
      type: InternetAddressType.IPv4,
      includeLoopback: false,
    );
    return {
      for (final i in ifaces)
        for (final a in i.addresses) a.address,
    }.toList();
  } catch (_) {
    return const [];
  }
}
