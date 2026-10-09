// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/design/app_design_theme.dart';

/// Enveloppe un widget dans un `MaterialApp` thémé pour comparaison
/// golden (ADR-0021 §9) — taille de surface fixe pour le déterminisme,
/// thème clair/sombre selon [brightness]. Les polices réelles sont
/// chargées globalement par `test/flutter_test_config.dart`.
Future<void> pumpGolden(
  WidgetTester tester,
  Widget child, {
  Brightness brightness = Brightness.light,
  Size surfaceSize = const Size(1280, 1024),
}) async {
  tester.view.physicalSize = surfaceSize;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: brightness == Brightness.light
          ? AppDesignTheme.light()
          : AppDesignTheme.dark(),
      home: child,
    ),
  );
  await tester.pumpAndSettle();
}
