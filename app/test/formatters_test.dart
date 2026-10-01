// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/utils/byte_formatter.dart';
import 'package:onionbit_ui/core/utils/duration_formatter.dart';

void main() {
  group('ByteFormatter', () {
    test('formate les ordres de grandeur courants (EN par défaut)', () {
      expect(ByteFormatter.format(0), '0 B');
      expect(ByteFormatter.format(512), '512 B');
      expect(ByteFormatter.format(2048), '2.0 KiB');
      expect(ByteFormatter.format(5 * 1024 * 1024), '5.0 MiB');
      expect(ByteFormatter.format(1610612736), '1.5 GiB');
    });

    test('formate les ordres de grandeur courants (FR)', () {
      expect(ByteFormatter.format(0, 'fr'), '0 o');
      expect(ByteFormatter.format(512, 'fr'), '512 o');
      expect(ByteFormatter.format(2048, 'fr'), '2.0 Ko');
      expect(ByteFormatter.format(5 * 1024 * 1024, 'fr'), '5.0 Mo');
      expect(ByteFormatter.format(1610612736, 'fr'), '1.5 Go');
    });

    test('formate un débit', () {
      expect(ByteFormatter.formatRate(1536), '1.5 KiB/s');
      expect(ByteFormatter.formatRate(1536, 'fr'), '1.5 Ko/s');
    });
  });

  group('DurationFormatter', () {
    test('formatSeconds : cas courants', () {
      expect(DurationFormatter.formatSeconds(-5), '—');
      expect(DurationFormatter.formatSeconds(0), '—');
      expect(DurationFormatter.formatSeconds(59), '59 s');
      expect(DurationFormatter.formatSeconds(65), '1 min 5 s');
      expect(DurationFormatter.formatSeconds(3660), '1 h 1 min');
    });

    test('formatSeconds : jours selon la locale', () {
      expect(DurationFormatter.formatSeconds(90061), '1 d 1 h');
      expect(DurationFormatter.formatSeconds(90061, 'fr'), '1 j 1 h');
    });

    test('formatSeconds : ETA astronomique (débit nul) → tiret', () {
      // Backend : eta = total / max(speed, 1e-6) — ~1e15 s quand le
      // débit est nul ; Duration(seconds:) débordait en négatif.
      expect(DurationFormatter.formatSeconds(2.9e15), '—');
      expect(DurationFormatter.formatSeconds(double.infinity), '—');
      expect(DurationFormatter.formatSeconds(double.nan), '—');
    });
  });
}
