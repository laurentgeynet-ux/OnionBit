import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'app_theme.dart';

/// Préférences d'apparence persistées (`shared_preferences`) :
/// couleur d'accent (seed `ColorScheme.fromSeed`) + mode de thème.
/// Clés de stockage — une seule source de vérité.
const _kKeySeedColor = 'theme.seedColor';
const _kKeyThemeMode = 'theme.themeMode';

/// Palette d'accents proposée dans les réglages (violet OnionBit par
/// défaut — couleur du logo).
const kAccentChoices = <(String, Color)>[
  ('OnionBit', Color(0xFF6C2EA6)),
  ('Bleu', Color(0xFF2F6FED)),
  ('Indigo', Color(0xFF5C6BC0)),
  ('Vert', Color(0xFF2E7D57)),
  ('Orange', Color(0xFFE07B39)),
  ('Rose', Color(0xFFC04B76)),
  ('Ardoise', Color(0xFF546E7A)),
];

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
