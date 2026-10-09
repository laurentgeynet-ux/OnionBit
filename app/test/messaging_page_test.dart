// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart' show Override;
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/design_tokens.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_attachment.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_contact.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_conversation.dart';
import 'package:onionbit_ui/features/messaging/domain/messaging_message.dart';
import 'package:onionbit_ui/features/messaging/presentation/pages/messaging_page.dart';
import 'package:onionbit_ui/features/messaging/presentation/providers/messaging_providers.dart';
import 'package:onionbit_ui/features/messaging/presentation/widgets/conversation_view.dart';

import 'helpers/l10n.dart';

/// Messagerie adaptative (ADR-0021 §5, étape 74) : liste+détail en
/// `AdaptiveListDetail` à partir du palier `expanded` ; sous ce palier,
/// la liste OU le fil sélectionné en plein écran avec retour.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const conv = MessagingConversation(
    convId: 'aabbccdd00112233',
    kind: 'direct',
    name: '',
    state: 'active',
    createdAt: 1,
    unread: 0,
    lastTs: 0,
    peer: 'deadbeefdeadbeef',
    alias: 'Alice',
  );

  List<Override> overrides() => [
    messagingEnabledProvider.overrideWith((ref) async => true),
    messagingStatsProvider.overrideWith((ref) async => null),
    messagingContactsProvider.overrideWith(
      (ref) async => const <MessagingContact>[],
    ),
    messagingPendingProvider.overrideWith(
      (ref) async => const <MessagingContact>[],
    ),
    messagingConversationsProvider.overrideWith((ref) async => [conv]),
    messagingEventsProvider.overrideWith((ref) => const Stream.empty()),
    messagingEventsBridgeProvider.overrideWith((ref) {}),
    convHistoryProvider.overrideWith(
      (ref, convId) async => const <MessagingMessage>[],
    ),
    convAttachmentsProvider.overrideWith(
      (ref, convId) async => const <MessagingAttachment>[],
    ),
  ];

  Future<void> pumpPage(WidgetTester tester, Size size) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      ProviderScope(
        overrides: overrides(),
        child: l10nTestApp(const Scaffold(body: MessagingPage())),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('expanded : deux panneaux via AdaptiveListDetail', (
    tester,
  ) async {
    await pumpPage(tester, const Size(1280, 800));
    expect(tester.takeException(), isNull);
    expect(find.byType(AdaptiveListDetail), findsOneWidget);
    expect(find.byType(ConversationTabs), findsOneWidget);
    expect(find.text('Alice'), findsOneWidget);
  });

  testWidgets('compact : liste seule, puis fil plein écran avec retour', (
    tester,
  ) async {
    await pumpPage(tester, const Size(390, 844));

    // Liste seule — pas de deuxième panneau ni d'onglets.
    expect(find.byType(AdaptiveListDetail), findsNothing);
    expect(find.byType(ConversationTabs), findsNothing);
    expect(find.byType(ConversationView), findsNothing);
    expect(find.text('Alice'), findsOneWidget);

    // Ouvrir la conversation pousse le fil en plein écran.
    await tester.tap(find.text('Alice'));
    await tester.pumpAndSettle();
    expect(find.byType(ConversationView), findsOneWidget);
    expect(find.byIcon(Icons.arrow_back), findsOneWidget);

    // Retour → la liste réapparaît.
    await tester.tap(find.byIcon(Icons.arrow_back));
    await tester.pumpAndSettle();
    expect(find.byType(ConversationView), findsNothing);
    expect(find.text('Alice'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
