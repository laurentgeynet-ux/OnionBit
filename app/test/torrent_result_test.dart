// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/search/data/rest_search_repository.dart';
import 'package:onionbit_ui/features/search/domain/torrent_result.dart';

void main() {
  group('TorrentResult magnet', () {
    test('magnet sans trackers reste minimal', () {
      const r = TorrentResult(
        infohash: '0123456789abcdef0123456789abcdef01234567',
        name: 'test torrent',
        size: 1000,
        source: TorrentSource.local,
      );
      expect(
        r.magnet,
        'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=test%20torrent',
      );
    });

    test('magnet avec trackers concatène tous les tr encodés', () {
      const r = TorrentResult(
        infohash: '0123456789abcdef0123456789abcdef01234567',
        name: 'Ubuntu 24.04',
        size: 2000000,
        source: TorrentSource.local,
        trackers: [
          'udp://tracker.opentrackr.org:1337/announce',
          'http://tracker.example.com/announce',
        ],
      );
      expect(
        r.magnet,
        'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567'
        '&dn=Ubuntu%2024.04'
        '&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce'
        '&tr=http%3A%2F%2Ftracker.example.com%2Fannounce',
      );
    });

    test('parseRemoteResults extrait trackers et tracker_info', () {
      final results = RestSearchRepository.parseRemoteResults({
        'results': [
          {
            'infohash': 'aabbcc',
            'name': 'Remote item',
            'size': 500,
            'trackers': ['udp://t1:80'],
            'tracker_info': 'udp://t2:80',
          },
        ],
      });
      expect(results, hasLength(1));
      expect(results.first.trackers, ['udp://t1:80', 'udp://t2:80']);
      expect(results.first.magnet, contains('&tr=udp%3A%2F%2Ft1%3A80'));
      expect(results.first.magnet, contains('&tr=udp%3A%2F%2Ft2%3A80'));
    });
  });
}
