// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/primitives/adaptive_list_detail.dart';

void main() {
  Widget harness() => const MaterialApp(
    home: AdaptiveListDetail(list: Text('LISTE'), detail: Text('DETAIL')),
  );

  testWidgets('côte à côte à partir du palier expanded (défaut)', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1280, 800); // expanded
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(harness());

    expect(find.text('LISTE'), findsOneWidget);
    expect(find.text('DETAIL'), findsOneWidget);
    expect(find.byType(VerticalDivider), findsOneWidget);
  });

  testWidgets('liste seule en dessous du palier (medium)', (tester) async {
    tester.view.physicalSize = const Size(800, 700); // medium
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(harness());

    expect(find.text('LISTE'), findsOneWidget);
    expect(find.text('DETAIL'), findsNothing);
  });
}
