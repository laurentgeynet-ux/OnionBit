// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/core/pairing/pairing_api.dart';

/// `PairingApi.redeem` (étape 76) : l'appel part **sans** `X-Api-Key`
/// (le jeton du QR vaut authentification — voir `auth.rs`) et rend la
/// clé du daemon ; un jeton refusé lève `ApiException` 401.
void main() {
  test('redeem envoie le jeton sans clé API et rend api_key', () async {
    http.Request? seen;
    final client = MockClient((req) async {
      seen = req;
      return http.Response(
        jsonEncode({'api_key': 'cle-du-daemon'}),
        200,
        headers: {'content-type': 'application/json'},
      );
    });

    final key = await const PairingApi().redeem(
      'http://192.168.1.20:8085',
      'a1b2c3',
      httpClient: client,
    );

    expect(key, 'cle-du-daemon');
    expect(seen, isNotNull);
    expect(seen!.method, 'POST');
    expect(seen!.url.path, '/api/pairing/redeem');
    expect(seen!.url.host, '192.168.1.20');
    expect(seen!.url.port, 8085);
    expect(jsonDecode(seen!.body), {'token': 'a1b2c3'});
    // Jamais d'en-tête d'auth : l'appelant ne possède pas encore la
    // clé — c'est ce qu'il vient chercher.
    expect(seen!.headers.containsKey('X-Api-Key'), isFalse);
  });

  test('redeem : jeton refusé → ApiException 401', () async {
    final client = MockClient(
      (_) async => http.Response(
        jsonEncode({
          'error': {'handled': true, 'message': 'unauthorized'},
        }),
        401,
        headers: {'content-type': 'application/json'},
      ),
    );
    expect(
      () => const PairingApi().redeem(
        'http://10.0.0.2:8085',
        'mort',
        httpClient: client,
      ),
      throwsA(
        isA<ApiException>().having(
          (e) => e.statusCode,
          'statusCode',
          401,
        ),
      ),
    );
  });
}
