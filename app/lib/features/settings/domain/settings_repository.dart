// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Contrat du dépôt réglages (`/api/settings`, `/api/shutdown`).
abstract interface class SettingsRepository {
  /// Arbre de configuration effectif du daemon (miroir `GET /api/settings`).
  Future<Map<String, dynamic>> get();

  /// Met à jour les réglages applicables à chaud (`POST /api/settings`).
  Future<void> update(Map<String, dynamic> settings);

  /// Demande l'arrêt du daemon (`PUT /api/shutdown`).
  Future<void> shutdown();

  /// Espace disque du répertoire (`PUT /api/statistics/dirspace` ;
  /// `null` = dossier de téléchargement par défaut) → `{total, used,
  /// free}` en octets.
  Future<Map<String, int>> dirSpace({String? directory});

  /// Items découverts par les watchers RSS (`GET /api/rss`).
  Future<List<Map<String, dynamic>>> rssItems();

  /// Remplace la liste des flux surveillés (`PUT /api/rss`,
  /// application à chaud côté daemon).
  Future<void> setRssFeeds(List<String> urls);

  /// Versions connues du daemon (`GET /api/versioning/versions` →
  /// `{versions, current}`).
  Future<Map<String, dynamic>> versions();

  /// Sonde de mise à jour (`GET /api/versioning/versions/check` →
  /// `{new_version, has_version}`).
  Future<Map<String, dynamic>> checkVersion();

  /// `GET /api/identity` — clé publique IPv8 de l'identité courante.
  /// `null` si IPv8 désactivé.
  Future<String?> identityPublicKey();

  /// `POST /api/identity/export` — clé secrète hex (brute ou blob
  /// `OBID` si [password] non vide). Renvoie `{key, encrypted}`.
  Future<Map<String, dynamic>> identityExport({String? password});

  /// `POST /api/identity/restore` — écrase `ipv8_keypair.bin` ;
  /// la nouvelle identité est active au prochain démarrage.
  Future<void> identityRestore(String keyHex, {String? password});
}
