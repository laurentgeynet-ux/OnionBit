// This file is part of OnionBit.
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
  /// free}` en octets. [area] (`public`|`private`, ADR-0018) mesure
  /// la racine de la zone et prime sur [directory].
  Future<Map<String, int>> dirSpace({String? directory, String? area});

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

  /// `GET /api/identity` complet — `{state, seeded, mode, persistent,
  /// public_key?}` (ADR-0016 : `state` = ready/locked/pending).
  Future<Map<String, dynamic>> identityStatus();

  /// `GET /api/identity/recovery_phrase?lang=` — phrase BIP39 de 24
  /// mots de l'identité seedée. `null` sur identité legacy.
  Future<String?> identityRecoveryPhrase({String? lang});

  /// `POST /api/identity/export` — clé secrète hex (brute ou blob
  /// `OBID` si [password] non vide). Renvoie `{key, encrypted}`.
  Future<Map<String, dynamic>> identityExport({String? password});

  /// `POST /api/identity/restore` — clé `LibNaCLSK:` hex ou blob
  /// `OBID` ; `forceLegacy` requis sur install seedée (conversion
  /// explicite — la graine gagnerait sinon au prochain boot).
  Future<void> identityRestore(
    String keyHex, {
    String? password,
    bool forceLegacy = false,
  });

  /// `POST /api/identity/restore` par phrase BIP39 (24 mots, EN/FR) —
  /// installe `identity_seed.bin` et régénère les clés dérivées.
  Future<void> identityRestorePhrase(String phrase);

  /// `POST /api/identity/at_rest` — scelle/déscelle la graine `OBSK`
  /// sur disque (mot de passe exigé dans les deux sens).
  Future<void> identitySetAtRest({required bool enabled, required String password});

  /// `POST /api/identity/create` — résolution « nouvelle identité »
  /// du gate `pending` ; [password] non vide scelle la graine
  /// (`OBSK`) d'emblée.
  Future<void> identityCreate({String? password});

  /// `POST /api/identity/guest` — session invitée éphémère (aucun
  /// artefact disque, base mémoire).
  Future<void> identityGuest();

  /// `POST /api/identity/unlock` — déverrouille une graine `OBSK`
  /// (état `locked`). 400 = mot de passe incorrect, 429 = rate-limit.
  Future<void> identityUnlock(String password);

  /// `GET /api/private` — zone privée ADR-0018 : `{state: locked|
  /// mounted|guest, downloads: [...], orphans: {obd_groups, bitv}}`.
  /// Les entrées ne sont visibles que montées (manifeste `OBM`
  /// déchiffré).
  Future<Map<String, dynamic>> privateZone();

  /// `DELETE /api/private/orphans` — purge explicite des orphelins
  /// `.obd`/`.bitv` rapportés au montage (jamais automatique).
  Future<void> purgePrivateOrphans();
}
