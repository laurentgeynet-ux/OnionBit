// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/design_tokens.dart';
import 'package:onionbit_ui/features/messaging/data/messaging_dto.dart';
import 'package:onionbit_ui/features/messaging/presentation/widgets/plane_badge.dart';

import 'helpers/l10n.dart';

/// Pastilles de plan de transport (ADR-0029 §8 Q3, étape 125) :
/// parsing du `plane`/`planes`/`onion` exposés par l'API et
/// rendu des couleurs distinctes OnionBit/Tor/boîte aux lettres.
void main() {
  group('DTO — champs multi-plan', () {
    test('message : plane parsé, repli ipv8 pour les lignes legacy', () {
      final onion = {
        'id': 'aa',
        'direction': 'out',
        'status': 'sent',
        'plane': 'onion',
      }.toMessagingMessage();
      expect(onion.plane, 'onion');
      expect(onion.isOnion, isTrue);
      expect(onion.isMailbox, isFalse);

      final mailbox = {'id': 'bb', 'plane': 'mailbox'}
          .toMessagingMessage();
      expect(mailbox.isMailbox, isTrue);

      // Ligne antérieure à la migration v26 : pas de champ.
      final legacy = {'id': 'cc', 'direction': 'in', 'status': 'received'}
          .toMessagingMessage();
      expect(legacy.plane, 'ipv8');
      expect(legacy.isOnion, isFalse);
      expect(legacy.isMailbox, isFalse);
    });

    test('contact : onion, onion_verified et planes', () {
      final c = {
        'public_key': 'deadbeef',
        'state': 'active',
        'link': 'none',
        'onion': 'abc123.onion',
        'onion_verified': true,
        'planes': ['onionbit', 'onion'],
      }.toMessagingContact();
      expect(c.onion, 'abc123.onion');
      expect(c.onionVerified, isTrue);
      expect(c.hasOnion, isTrue);
      expect(c.planes, ['onionbit', 'onion']);

      // Sans association : plans = baseline OnionBit, hasOnion faux.
      final bare = {'public_key': 'deadbeef', 'state': 'active'}
          .toMessagingContact();
      expect(bare.onion, isNull);
      expect(bare.onionVerified, isFalse);
      expect(bare.hasOnion, isFalse);
      expect(bare.planes, ['onionbit']);
    });

    test('conversation : planes du correspondant', () {
      final conv = {
        'conv_id': 'aabb',
        'kind': 'direct',
        'peer': 'deadbeef',
        'onion': 'abc123.onion',
        'planes': ['onionbit', 'onion'],
      }.toMessagingConversation();
      expect(conv.planes, contains('onion'));

      final group = {
        'conv_id': 'ccdd',
        'kind': 'group',
      }.toMessagingConversation();
      expect(group.planes, isEmpty);
      expect(group.onion, isNull);
    });
  });

  group('PlaneDot', () {
    testWidgets('couleurs distinctes par plan', (tester) async {
      await tester.pumpWidget(
        l10nTestApp(
          const Scaffold(
            body: Row(
              children: [
                PlaneDot(plane: 'ipv8'),
                PlaneDot(plane: 'onion'),
                PlaneDot(plane: 'mailbox'),
                PlaneDot(plane: 'inconnu'),
              ],
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // Trois pastilles visibles ; le plan inconnu ne rend rien.
      final dots = tester
          .widgetList<Container>(find.byType(Container))
          .where((c) => c.decoration is BoxDecoration)
          .map((c) => (c.decoration! as BoxDecoration).color)
          .toList();
      expect(dots, contains(AppPlaneColors.onionbit));
      expect(dots, contains(AppPlaneColors.tor));
      expect(dots, contains(AppPlaneColors.mailbox));

      // Tooltips localisés présents dans l'arbre.
      expect(find.byType(Tooltip), findsNWidgets(3));
    });

    testWidgets('tooltip de substitution (liaison vérifiée)', (
      tester,
    ) async {
      await tester.pumpWidget(
        l10nTestApp(
          const Scaffold(
            body: PlaneDot(plane: 'onion', tooltip: 'liaison vérifiée'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final tip = tester.widget<Tooltip>(find.byType(Tooltip));
      expect(tip.message, 'liaison vérifiée');
    });
  });

  group('PlaneBadges', () {
    testWidgets('un point par plan connu, dans l’ordre du DTO', (
      tester,
    ) async {
      await tester.pumpWidget(
        l10nTestApp(
          const Scaffold(
            body: PlaneBadges(planes: ['onionbit', 'onion', '???']),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(PlaneDot), findsNWidgets(2));
    });
  });
}
