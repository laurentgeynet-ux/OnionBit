// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/diagnostic/domain/diagnostic_models.dart';
import 'package:onionbit_ui/features/diagnostic/presentation/providers/diagnostic_providers.dart';

/// Source mutable injectée à la place de `ipv8TrafficProvider` :
/// chaque `push()` émet une nouvelle donnée.
class _TrafficSource extends Notifier<Ipv8Traffic> {
  @override
  Ipv8Traffic build() => const Ipv8Traffic(up: 0, down: 0);

  void push(Ipv8Traffic v) => state = v;
}

final _source = NotifierProvider<_TrafficSource, Ipv8Traffic>(
  _TrafficSource.new,
);

/// Au-delà de `_kMinSampleWindow` (1 s) : deux échantillons
/// espacés ainsi publient toujours un débit.
const _gap = Duration(milliseconds: 1100);

/// Juste assez pour laisser l'émission `FutureProvider` arriver.
const _settle = Duration(milliseconds: 50);

void main() {
  test(
    'tunnelTrafficProvider : baseline, débit, émissions groupées, reset',
    () async {
      final container = ProviderContainer.test(
        overrides: [
          ipv8TrafficProvider.overrideWith(
            (ref) async => ref.watch(_source),
          ),
        ],
      );
      addTearDown(container.dispose);

      // Sans listener, un provider `autoDispose` est recréé à
      // chaque `read` — la souscription imite le `ref.watch` de
      // la carte.
      final sub = container.listen(tunnelTrafficProvider, (_, _) {});
      addTearDown(sub.close);

      TunnelTraffic read() => container.read(tunnelTrafficProvider);
      void push(Ipv8Traffic v) => container.read(_source.notifier).push(v);

      // Premier échantillon = baseline : cumuls visibles, pas de
      // débit.
      expect(read().hasRate, isFalse);
      push(const Ipv8Traffic(up: 1000, down: 2000));
      await Future<void>.delayed(_settle);
      expect(read().totalUp, 1000);
      expect(read().totalDown, 2000);
      expect(read().hasRate, isFalse);

      // Compteurs en hausse un tick plus tard -> débit positif
      // dans les deux sens.
      await Future<void>.delayed(_gap);
      push(const Ipv8Traffic(up: 11000, down: 32000));
      await Future<void>.delayed(_settle);
      var t = read();
      expect(t.hasRate, isTrue);
      final rateUp = t.rateUp!;
      final rateDown = t.rateDown!;
      expect(rateUp, greaterThan(0));
      expect(rateDown, greaterThan(rateUp));
      expect(t.totalUp, 11000);
      expect(t.totalDown, 32000);

      // Émission groupée (< 1 s après la précédente, delta ~0) :
      // ignorée — le débit publié n'est pas écrasé.
      push(const Ipv8Traffic(up: 11005, down: 32005));
      await Future<void>.delayed(_settle);
      t = read();
      expect(t.rateUp, rateUp);
      expect(t.rateDown, rateDown);

      // Compteurs remis à zéro (restart daemon) -> débit borné à 0.
      await Future<void>.delayed(_gap);
      push(const Ipv8Traffic(up: 10, down: 20));
      await Future<void>.delayed(_settle);
      expect(read().rateUp, 0);
      expect(read().rateDown, 0);
      expect(read().totalUp, 10);
    },
    timeout: const Timeout(Duration(seconds: 15)),
  );
}
