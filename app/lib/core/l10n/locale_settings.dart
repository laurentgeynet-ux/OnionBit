// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:ui';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Langue de l'interface. `system` suit la locale de l'OS ; `en` est le
/// défaut de l'application (gabarit `app_en.arb`).
enum AppLocale { system, en, fr }

/// Préférence de langue persistée (`shared_preferences`) — clé
/// `ui.locale`, défaut [AppLocale.en]. Appliquée à `MaterialApp.locale`
/// à la racine (`null` en mode `system` pour laisser l'OS trancher).
final localeSettingsProvider =
    AsyncNotifierProvider<LocaleSettingsNotifier, AppLocale>(
      LocaleSettingsNotifier.new,
    );

class LocaleSettingsNotifier extends AsyncNotifier<AppLocale> {
  static const _kKey = 'ui.locale';

  @override
  Future<AppLocale> build() async {
    final prefs = await SharedPreferences.getInstance();
    return AppLocale.values.asNameMap()[prefs.getString(_kKey)] ??
        AppLocale.en;
  }

  Future<void> setLocale(AppLocale locale) async {
    state = AsyncData(locale);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kKey, locale.name);
  }
}

/// Résolution `AppLocale` → `Locale` Flutter (`null` = délégué à l'OS).
Locale? resolveFlutterLocale(AppLocale? locale) => switch (locale) {
  AppLocale.en => const Locale('en'),
  AppLocale.fr => const Locale('fr'),
  _ => null,
};
