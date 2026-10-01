// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';

/// `MaterialApp` de test câblé avec les délégués de localisation —
/// indispensable pour tout widget utilisant `context.l10n`. La locale
/// testée est passée en paramètre (défaut : anglais).
Widget l10nTestApp(Widget home, {Locale locale = const Locale('en')}) {
  return MaterialApp(
    locale: locale,
    localizationsDelegates: AppLocalizations.localizationsDelegates,
    supportedLocales: AppLocalizations.supportedLocales,
    home: home,
  );
}
