// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_repository.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';

/// Репозиторий с подменёнными счётчиками: отмечает первый опрос, чтобы
/// тест детерминированно ставил baseline. Остальные методы тесту не
/// нужны — `noSuchMethod` вернёт null при случайном вызове.
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
    'tunnelTrafficRateProvider diff сондирований: дебит, сброс счётчиков -> 0',
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

      // Baseline поставлен первым опросом; счётчики выросли -> дебит.
      await repo.polled.future;
      repo.sample = const Ipv8Traffic(up: 30000, down: 60000);
      await _until(() => rates.isNotEmpty);
      expect(rates, hasLength(1));
      expect(rates.single.up, greaterThan(0));
      expect(rates.single.down, greaterThan(rates.single.up));

      // Счётчики обнулились (restart daemon) -> без отрицательного дебита.
      repo.sample = const Ipv8Traffic(up: 10, down: 20);
      await _until(() => rates.length > 1);
      expect(rates.last.up, 0);
      expect(rates.last.down, 0);
    },
    timeout: const Timeout(Duration(seconds: 30)),
  );
}
