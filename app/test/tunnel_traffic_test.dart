// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';

/// Source mutable injectée à la place de `ipv8TrafficProvider` :
/// chaque `set()` émet une nouvelle donnée — le débit du notifier
/// est difficile à dater (compteurs réels), mais sa séquence
/// baseline → débit positif → clamp à 0 est déterministe.
class _TrafficSource extends Notifier<Ipv8Traffic> {
  @override
  Ipv8Traffic build() => const Ipv8Traffic(up: 0, down: 0);

  void push(Ipv8Traffic v) => state = v;
}

final _source = NotifierProvider<_TrafficSource, Ipv8Traffic>(
  _TrafficSource.new,
);

void main() {
  test('tunnelTrafficProvider : baseline, débit positif, reset -> 0', () async {
    final container = ProviderContainer.test(
      overrides: [
        ipv8TrafficProvider.overrideWith(
          (ref) async => ref.watch(_source),
        ),
      ],
    );
    addTearDown(container.dispose);

    // Sans listener, un provider `autoDispose` est recréé à chaque
    // `read` — la souscription imite le `ref.watch` de la carte.
    final sub = container.listen(tunnelTrafficProvider, (_, _) {});
    addTearDown(sub.close);

    TunnelTraffic read() => container.read(tunnelTrafficProvider);
    void push(Ipv8Traffic v) => container.read(_source.notifier).push(v);

    // Premier échantillon = baseline : cumuls visibles, pas de débit.
    expect(read().hasRate, isFalse);
    push(const Ipv8Traffic(up: 1000, down: 2000));
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(read().totalUp, 1000);
    expect(read().totalDown, 2000);
    expect(read().hasRate, isFalse);

    // Compteurs en hausse -> débit positif dans les deux sens.
    push(const Ipv8Traffic(up: 11000, down: 32000));
    await Future<void>.delayed(const Duration(milliseconds: 20));
    final t = read();
    expect(t.hasRate, isTrue);
    expect(t.rateUp, greaterThan(0));
    expect(t.rateDown, greaterThan(t.rateUp!));
    expect(t.totalUp, 11000);
    expect(t.totalDown, 32000);

    // Compteurs remis à zéro (restart daemon) -> débit borné à 0.
    push(const Ipv8Traffic(up: 10, down: 20));
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(read().rateUp, 0);
    expect(read().rateDown, 0);
    expect(read().totalUp, 10);
  });
}
