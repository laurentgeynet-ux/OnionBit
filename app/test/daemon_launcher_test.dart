// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/config/app_config.dart';
import 'package:onionbit_ui/core/config/daemon_launcher_native.dart';

void main() {
  group('isDaemonApiAlive', () {
    test('toute réponse HTTP (même 401) signale un daemon vivant', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      addTearDown(server.close);
      server.listen((req) {
        req.response.statusCode = HttpStatus.unauthorized;
        req.response.close();
      });
      final config = AppConfig(
        baseUrl: 'http://127.0.0.1:${server.port}',
        apiKey: 'cle-de-test',
      );
      expect(await isDaemonApiAlive(config), isTrue);
    });

    test('port sans écoute → daemon absent', () async {
      // Bind puis fermeture immédiate : le port est garanti mort.
      final probe = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final deadPort = probe.port;
      await probe.close();
      final config = AppConfig(baseUrl: 'http://127.0.0.1:$deadPort');
      expect(await isDaemonApiAlive(config), isFalse);
    });
  });
}
