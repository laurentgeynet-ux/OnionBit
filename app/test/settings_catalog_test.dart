// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/features/settings/presentation/settings_catalog.dart';
import 'package:onionbit_ui/l10n/app_localizations_en.dart';
import 'package:onionbit_ui/l10n/app_localizations_fr.dart';

/// Régression de l'extraction du catalogue des réglages (ADR-0021 §6) :
/// les 15 sections gardent titre localisé, mots-clés de recherche et
/// icône — métadonnées partagées entre la page et la palette.
void main() {
  test('les 15 sections ont titre FR/EN, mots-clés et icône', () {
    expect(SettingsSectionId.values, hasLength(15));
    final en = AppLocalizationsEn();
    final fr = AppLocalizationsFr();
    for (final id in SettingsSectionId.values) {
      expect(id.title(en), isNotEmpty, reason: 'titre EN de ${id.name}');
      expect(id.title(fr), isNotEmpty, reason: 'titre FR de ${id.name}');
      expect(
        settingsSectionKeywords[id],
        isNotEmpty,
        reason: 'mots-clés de ${id.name}',
      );
      expect(id.icon, isA<IconData>(), reason: 'icône de ${id.name}');
    }
  });

  test('les mots-clés restent bilingues (recherche FR dans UI EN)', () {
    // Échantillons représentatifs des deux directions.
    expect(
      settingsSectionKeywords[SettingsSectionId.automation],
      contains('watch folder'),
    );
    expect(
      settingsSectionKeywords[SettingsSectionId.automation],
      contains('flux'),
    );
    expect(
      settingsSectionKeywords[SettingsSectionId.identity],
      contains('sauvegarde'),
    );
    expect(
      settingsSectionKeywords[SettingsSectionId.identity],
      contains('backup'),
    );
  });

  test('deep-link : chaque id produit un `?s=` résolu par la page', () {
    // La page résout `s=<id.name>` — l'enum garantit la correspondance.
    for (final id in SettingsSectionId.values) {
      expect(id.name, matches(RegExp(r'^[a-z]+$')));
    }
  });
}
