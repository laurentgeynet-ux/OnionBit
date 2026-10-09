// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../l10n/app_localizations.dart' show AppLocalizations;

/// Groupe d'une commande dans la palette (ADR-0021 §6) — ordre
/// d'affichage et étiquette localisée de regroupement.
enum CommandGroup {
  navigation,
  messaging,
  settings,
  actions;

  /// Libellé localisé de l'en-tête de groupe.
  String label(AppLocalizations l10n) => switch (this) {
    CommandGroup.navigation => l10n.cmdGroupNavigation,
    CommandGroup.messaging => l10n.cmdGroupMessaging,
    CommandGroup.settings => l10n.cmdGroupSettings,
    CommandGroup.actions => l10n.cmdGroupActions,
  };
}

/// Une entrée du catalogue de commandes : titre localisé, mots-clés
/// de recherche (généralisation des `keywords` des sections de
/// réglages — métadonnées non affichées) et [run], l'action à exécuter
/// dans le contexte de navigation de la palette.
class AppCommand {
  const AppCommand({
    required this.id,
    required this.group,
    required this.icon,
    required this.title,
    this.subtitle,
    this.keywords = '',
    required this.run,
  });

  /// Identifiant stable (ex. `nav.downloads`, `conv.<hex>`,
  /// `settings.<section>`) — utilisé comme clé de widget et pour les
  /// tests, jamais affiché.
  final String id;

  /// Groupe d'affichage — les commandes sont rendues dans l'ordre de
  /// [CommandGroup.values].
  final CommandGroup group;

  final IconData icon;

  /// Titre localisé affiché dans la palette.
  final String title;

  /// Contexte secondaire affiché à droite du titre (ex. section parente).
  final String? subtitle;

  /// Mots-clés de recherche supplémentaires — concaténés au titre pour
  /// la correspondance, jamais affichés (bilingue conseillé : taper en
  /// français dans une UI anglaise doit marcher).
  final String keywords;

  /// Exécute la commande (navigation, ouverture d'onglet, dialogue…).
  /// `ref` permet de lire/écrire les providers (ex. sélectionner une
  /// conversation) — la palette est fermée avant l'appel.
  final void Function(BuildContext context, WidgetRef ref) run;

  /// Correspondance insensible à la casse sur titre + sous-titre +
  /// mots-clés — même logique que l'ancien filtre des réglages.
  bool matches(String query) {
    if (query.isEmpty) return true;
    return '$title ${subtitle ?? ''} $keywords'
        .toLowerCase()
        .contains(query.toLowerCase());
  }
}
