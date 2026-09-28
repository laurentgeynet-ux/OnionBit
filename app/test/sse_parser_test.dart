import 'package:flutter_test/flutter_test.dart';
import 'package:tribler_ui/core/api/events.dart';
import 'package:tribler_ui/core/api/sse_client.dart';

void main() {
  group('SseEventParser', () {
    test('parse un événement nommé avec data JSON', () async {
      const body =
          'event: download_state_changed\n'
          'data: {"infohash":"abc","status":"DOWNLOADING"}\n\n';

      final events = await Stream.value(body)
          .transform(const SseEventParser())
          .toList();

      expect(events, hasLength(1));
      expect(events.single.topic, EventTopics.downloadStateChanged);
      expect(events.single.data['infohash'], 'abc');
    });

    test('découpe les événements multiples et concatène les data', () async {
      const body =
          'data: {"a":1}\n'
          'data: {"b":2}\n\n'
          'event: ping\n'
          'data: {"x":true}\n\n';

      final events = await Stream.value(body)
          .transform(const SseEventParser())
          .toList();

      expect(events, hasLength(2));
      expect(events[0].topic, ''); // événement anonyme
      expect(events[1].topic, 'ping');
      expect(events[1].data['x'], isTrue);
    });

    test('ignore les keep-alive et les commentaires', () async {
      const body = ': keepalive\n\nevent: x\ndata: {}\n\n';
      final events = await Stream.value(body)
          .transform(const SseEventParser())
          .toList();
      expect(events, hasLength(1));
      expect(events.single.topic, 'x');
    });

    test('tolère un événement scindé entre deux chunks', () async {
      final events = await Stream.fromIterable([
        'event: torre',
        'nt_finished\ndata: {"i',
        'h":"a"}\n\n',
      ]).transform(const SseEventParser()).toList();
      expect(events, hasLength(1));
      expect(events.single.topic, EventTopics.torrentFinished);
      expect(events.single.data['ih'], 'a');
    });
  });
}
