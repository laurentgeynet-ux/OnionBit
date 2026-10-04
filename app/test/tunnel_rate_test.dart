// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_repository.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';

/// Dépôt factice à compteurs pilotés : le premier sondage est
/// signalé pour que le test pose sa baseline de façon déterministe.
/// Les autres méthodes sont inutilisées — `noSuchMethod` renvoie
/// null si le hasard les appelait.
class _FakeDiagnosticRepo implements DiagnosticRepository {
  var sample = const Ipv8Traffic(up: 0, down: 0);
  final polled = Completer<void>();

  @override
  Future<Ipv8Traffic> ipv8Traffic() async {
    if (!polled.isCompleted) {
      polled.complete();
    }
    return sample;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

Future<void> _until(bool Function() ok, {int tries = 300}) async {
  for (var i = 0; i < tries && !ok(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
}

void main() {
  test(
    'tunnelTrafficRateProvider : diff de sondages, compteurs remis à zéro -> 0',
    () async {
      final repo = _FakeDiagnosticRepo();
      final container = ProviderContainer.test(
        overrides: [
          diagnosticRepositoryProvider.overrideWithValue(repo),
        ],
      );
      addTearDown(container.dispose);

      final rates = <({int down, int up})>[];
      final sub = container.listen(
        tunnelTrafficRateProvider,
        (_, next) {
          final v = next.value;
          if (v != null) {
            rates.add(v);
          }
        },
      );
      addTearDown(sub.close);

      // `build()` émet d'abord la valeur initiale (0,0) ; le premier
      // sondage pose la baseline sans débit. Les compteurs ont grandi
      // -> un débit positif finit par être émis.
      await repo.polled.future;
      repo.sample = const Ipv8Traffic(up: 30000, down: 60000);
      await _until(() => rates.any((r) => r.up > 0));
      final positive = rates.firstWhere((r) => r.up > 0);
      expect(positive.down, greaterThan(positive.up));

      // Compteurs remis à zéro (restart du daemon) -> pas de débit
      // négatif.
      final seen = rates.length;
      repo.sample = const Ipv8Traffic(up: 10, down: 20);
      await _until(() => rates.length > seen);
      expect(rates.last.up, 0);
      expect(rates.last.down, 0);
    },
    timeout: const Timeout(Duration(seconds: 30)),
  );
}
