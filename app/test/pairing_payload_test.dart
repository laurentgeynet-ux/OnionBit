// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/pairing/pairing_payload.dart';

/// Contrat du QR d'appairage (étape 76) : `onionbit://pair/1?host=…&
/// port=…&token=…` — l'allers-retours encode→parse doit être strict,
/// toute forme non conforme rend `null` (rien d'exploitable).
void main() {
  test('encode → tryParse : aller-retour fidèle', () {
    const p = PairingPayload(
      host: '192.168.1.20',
      port: 8085,
      token: 'a1b2c3',
    );
    final decoded = PairingPayload.tryParse(p.encode());
    expect(decoded, isNotNull);
    expect(decoded!.host, '192.168.1.20');
    expect(decoded.port, 8085);
    expect(decoded.token, 'a1b2c3');
    expect(decoded.baseUrl, 'http://192.168.1.20:8085');
  });

  test('encode échappe les caractères spéciaux (hôte nommé)', () {
    const p = PairingPayload(
      host: 'mon poste.local',
      port: 9000,
      token: 'ff00',
    );
    final decoded = PairingPayload.tryParse(p.encode());
    expect(decoded?.host, 'mon poste.local');
  });

  group('tryParse rejette', () {
    for (final (name, raw) in [
      ('chaîne vide', ''),
      ('QR étranger', 'https://example.com/qr'),
      ('mauvais schéma', 'http://pair/1?host=h&port=1&token=t'),
      ('mauvais hôte path', 'onionbit://other/1?host=h&port=1&token=t'),
      ('version inconnue', 'onionbit://pair/2?host=h&port=1&token=t'),
      ('sans version', 'onionbit://pair?host=h&port=1&token=t'),
      ('hôte manquant', 'onionbit://pair/1?port=8085&token=t'),
      ('port manquant', 'onionbit://pair/1?host=h&token=t'),
      ('port non numérique', 'onionbit://pair/1?host=h&port=x&token=t'),
      ('port hors bornes', 'onionbit://pair/1?host=h&port=70000&token=t'),
      ('token absent', 'onionbit://pair/1?host=h&port=8085'),
      ('token vide', 'onionbit://pair/1?host=h&port=8085&token='),
    ]) {
      test(name, () => expect(PairingPayload.tryParse(raw), isNull));
    }
  });
}
