// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart' show IconData, Icons;

import '../../../l10n/app_localizations.dart' show AppLocalizations;

/// Identifiants stables des sections de réglages — publics parce que
/// partagés entre `settings_page.dart` (ancres + filtre) et la palette
/// de commandes (ADR-0021 §6 : deep-link `/settings?s=<id.name>`).
enum SettingsSectionId {
  appearance,
  downloads,
  storage,
  bandwidth,
  queue,
  seeding,
  anonymity,
  onionbit,
  stealth,
  identity,
  network,
  automation,
  versioning,
  connection,
  daemon,
}

extension SettingsSectionIdX on SettingsSectionId {
  /// Titre localisé de la section (puce d'ancre, filtre, palette).
  String title(AppLocalizations l10n) => switch (this) {
    SettingsSectionId.appearance => l10n.settingsAppearanceTitle,
    SettingsSectionId.downloads => l10n.sectionDownloadsDefaults,
    SettingsSectionId.storage => l10n.sectionStorage,
    SettingsSectionId.bandwidth => l10n.sectionBandwidth,
    SettingsSectionId.queue => l10n.sectionQueue,
    SettingsSectionId.seeding => l10n.sectionSeeding,
    SettingsSectionId.anonymity => l10n.sectionAnonymity,
    SettingsSectionId.onionbit => l10n.sectionOnionBit,
    SettingsSectionId.stealth => l10n.sectionStealth,
    SettingsSectionId.identity => l10n.sectionIdentity,
    SettingsSectionId.network => l10n.sectionNetwork,
    SettingsSectionId.automation => l10n.sectionAutomation,
    SettingsSectionId.versioning => l10n.sectionVersioning,
    SettingsSectionId.connection => l10n.sectionConnection,
    SettingsSectionId.daemon => l10n.sectionDaemon,
  };

  /// Icône de la section — affichée dans la palette de commandes
  /// (ADR-0021 §6) ; les puces d'ancre de la page restent sans icône.
  IconData get icon => switch (this) {
    SettingsSectionId.appearance => Icons.palette_outlined,
    SettingsSectionId.downloads => Icons.download_outlined,
    SettingsSectionId.storage => Icons.folder_outlined,
    SettingsSectionId.bandwidth => Icons.speed_outlined,
    SettingsSectionId.queue => Icons.format_list_numbered_outlined,
    SettingsSectionId.seeding => Icons.upload_outlined,
    SettingsSectionId.anonymity => Icons.shield_outlined,
    SettingsSectionId.onionbit => Icons.hub_outlined,
    SettingsSectionId.stealth => Icons.visibility_off_outlined,
    SettingsSectionId.identity => Icons.key_outlined,
    SettingsSectionId.network => Icons.lan_outlined,
    SettingsSectionId.automation => Icons.smart_toy_outlined,
    SettingsSectionId.versioning => Icons.system_update_outlined,
    SettingsSectionId.connection => Icons.link_outlined,
    SettingsSectionId.daemon => Icons.dns_outlined,
  };
}

/// Catégories de regroupement des 15 sections (ADR-0021, étape 75 —
/// « réglages par paliers ») : l'énumération ordonne l'affichage,
/// chaque section déclare sa catégorie via
/// [SettingsSectionCategoryX.category].
enum SettingsCategory {
  essentials,
  networkAnonymity,
  automation,
  advanced;

  /// Titre localisé de l'en-tête repliable.
  String title(AppLocalizations l10n) => switch (this) {
    SettingsCategory.essentials => l10n.settingsCatEssentials,
    SettingsCategory.networkAnonymity => l10n.settingsCatNetworkAnonymity,
    SettingsCategory.automation => l10n.settingsCatAutomation,
    SettingsCategory.advanced => l10n.settingsCatAdvanced,
  };

  /// Icône de l'en-tête de catégorie.
  IconData get icon => switch (this) {
    SettingsCategory.essentials => Icons.star_outline,
    SettingsCategory.networkAnonymity => Icons.vpn_lock_outlined,
    SettingsCategory.automation => Icons.bolt_outlined,
    SettingsCategory.advanced => Icons.tune,
  };
}

/// Catégorie d'appartenance de chaque section — même source pour la
/// page (groupes repliables) et la palette de commandes.
extension SettingsSectionCategoryX on SettingsSectionId {
  SettingsCategory get category => switch (this) {
    SettingsSectionId.appearance ||
    SettingsSectionId.downloads ||
    SettingsSectionId.identity ||
    SettingsSectionId.connection => SettingsCategory.essentials,
    SettingsSectionId.anonymity ||
    SettingsSectionId.stealth ||
    SettingsSectionId.onionbit ||
    SettingsSectionId.network => SettingsCategory.networkAnonymity,
    SettingsSectionId.queue ||
    SettingsSectionId.bandwidth ||
    SettingsSectionId.seeding ||
    SettingsSectionId.automation ||
    SettingsSectionId.versioning => SettingsCategory.automation,
    SettingsSectionId.storage ||
    SettingsSectionId.daemon => SettingsCategory.advanced,
  };
}

/// Mots-clés de recherche par section — termes FR+EN, noms de champs
/// et chemins de clés (jamais affichés ; la correspondance est une
/// sous-chaîne insensible à la casse, comme le filtre historique).
/// Généralisation du champ `keywords` des anciens `_SectionEntry`
/// privés pour la palette de commandes (ADR-0021 §6).
const Map<SettingsSectionId, String> settingsSectionKeywords = {
  SettingsSectionId.appearance:
      'thème mode clair sombre accent couleur theme light dark '
      'color language langue',
  SettingsSectionId.downloads:
      'destination dossier espace disque download_defaults saveas '
      'folder disk space default',
  SettingsSectionId.storage:
      'stockage storage zones public private privé chiffré '
      'move_on_completion default_area private_enabled espace '
      'portable orphelins orphans obd',
  SettingsSectionId.bandwidth:
      'limite débit vitesse ko/s max_download_rate max_upload_rate '
      'limit rate speed kb/s',
  SettingsSectionId.queue:
      'queue active_downloads active_seeds active_checking '
      'active_limit auto_managed fastresume vérification démarrage '
      'startup check',
  SettingsSectionId.seeding:
      'seeding ratio durée hops sauts safe seeding '
      'download_defaults number_anon_downloads duration default',
  SettingsSectionId.anonymity:
      'tunnel community circuits min_circuits max_circuits '
      'exitnode sortie test vitesse exit speed anonymous',
  SettingsSectionId.onionbit:
      'onionbit ext extension ledger registre comptabilité '
      'accounting obf obfuscation curateurs curators trust '
      'confiance attest sign-then-serve enforce msg_v1',
  SettingsSectionId.stealth:
      'stealth furtif censure censure-resistant pont bridge '
      'onionbit-bridge invitation cover traffic camouflage '
      'role client gateway passerelle',
  SettingsSectionId.identity:
      'identité identity clé key export import backup sauvegarde '
      'restaurer restore obid ipv8_keypair nomade portable',
  SettingsSectionId.network:
      'dht upnp natpmp lsd utp proxy socks port écoute '
      'listen_interface listen',
  SettingsSectionId.automation:
      'watch folder rss flux dossier surveillance items feed',
  SettingsSectionId.versioning:
      'version mise à jour update checker versioning upgrade',
  SettingsSectionId.connection:
      'daemon clé api port http connexion key url',
  SettingsSectionId.daemon:
      'arrêt shutdown redémarrage logs stop restart state',
};
