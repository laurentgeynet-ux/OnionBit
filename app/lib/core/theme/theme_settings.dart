// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../l10n/app_localizations.dart' show AppLocalizations;
import 'app_theme.dart';

/// Préférences d'apparence persistées (`shared_preferences`) :
/// couleur d'accent (seed `ColorScheme.fromSeed`) + mode de thème.
/// Clés de stockage — une seule source de vérité.
const _kKeySeedColor = 'theme.seedColor';
const _kKeyThemeMode = 'theme.themeMode';

/// Identifiants d'accent — les libellés affichés sont localisés via
/// [AccentIdX.label] (ARB), jamais stockés en dur.
enum AccentId { onionbit, blue, indigo, green, orange, pink, slate }

/// Palette d'accents proposée dans les réglages (violet OnionBit par
/// défaut — couleur du logo).
const kAccentChoices = <(AccentId, Color)>[
  (AccentId.onionbit, Color(0xFF6C2EA6)),
  (AccentId.blue, Color(0xFF2F6FED)),
  (AccentId.indigo, Color(0xFF5C6BC0)),
  (AccentId.green, Color(0xFF2E7D57)),
  (AccentId.orange, Color(0xFFE07B39)),
  (AccentId.pink, Color(0xFFC04B76)),
  (AccentId.slate, Color(0xFF546E7A)),
];

extension AccentIdX on AccentId {
  /// Libellé localisé de l'accent (`'OnionBit'` est une marque,
  /// non traduite).
  String label(AppLocalizations l10n) => switch (this) {
    AccentId.onionbit => 'OnionBit',
    AccentId.blue => l10n.accentBlue,
    AccentId.indigo => l10n.accentIndigo,
    AccentId.green => l10n.accentGreen,
    AccentId.orange => l10n.accentOrange,
    AccentId.pink => l10n.accentPink,
    AccentId.slate => l10n.accentSlate,
  };
}

final themeSettingsProvider =
    AsyncNotifierProvider<ThemeSettingsNotifier, ThemeSettings>(
      ThemeSettingsNotifier.new,
    );

class ThemeSettings {
  const ThemeSettings({required this.seedColor, required this.mode});

  final Color seedColor;
  final ThemeMode mode;
}

class ThemeSettingsNotifier extends AsyncNotifier<ThemeSettings> {
  @override
  Future<ThemeSettings> build() async {
    final prefs = await SharedPreferences.getInstance();
    return ThemeSettings(
      seedColor: Color(
        prefs.getInt(_kKeySeedColor) ?? AppTheme.defaultSeedColor.toARGB32(),
      ),
      mode:
          ThemeMode.values.asNameMap()[prefs.getString(_kKeyThemeMode)] ??
          ThemeMode.system,
    );
  }

  Future<void> setSeedColor(Color color) async {
    final current = state.value;
    if (current == null) return;
    final next = ThemeSettings(seedColor: color, mode: current.mode);
    state = AsyncData(next);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kKeySeedColor, color.toARGB32());
  }

  Future<void> setMode(ThemeMode mode) async {
    final current = state.value;
    if (current == null) return;
    final next = ThemeSettings(seedColor: current.seedColor, mode: mode);
    state = AsyncData(next);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kKeyThemeMode, mode.name);
  }
}
