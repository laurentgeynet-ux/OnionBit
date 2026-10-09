// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'privacy_profile.dart';

/// Contrat du dépôt profil d'anonymat (`/api/privacy/profile`,
/// `/api/stealth/bridges`, `/api/shutdown` — ADR-0022).
abstract class PrivacyRepository {
  /// `GET /api/privacy/profile` — posture persistée + dérivée.
  Future<PrivacyProfileState> profile();

  /// `PUT /api/privacy/profile` `{profile}` — bascule de posture.
  /// Réponse : `{modified, effective, diverged_keys,
  /// restart_required, applied_keys}`.
  ///
  /// Erreurs `ApiException` : `400` profil inconnu, `409` avec
  /// `message == 'missing_prerequisites'` (`full` sans pont) ou
  /// `guest_session` (session invitée).
  Future<PrivacySwitchResult> switchProfile(PrivacyProfileKind target);

  /// `POST /api/stealth/bridges` `{link}` — ajoute un pont par lien
  /// d'invitation `onionbit-bridge://` (prérequis du mode `full`).
  Future<void> addBridge(String link);

  /// `PUT /api/shutdown` — arrêt du daemon : la reconnexion
  /// (`connectionWatchdog` + `ensureDaemonRunning`, daemon local)
  /// rejoue le spawn — cycle de redémarrage ADR-0022 §4.
  Future<void> shutdown();
}

/// Résultat de `switchProfile` — la bascule a réussi ; le serveur
/// indique si un redémarrage est nécessaire pour les clés froides.
class PrivacySwitchResult {
  const PrivacySwitchResult({
    required this.modified,
    required this.restartRequired,
  });

  /// `true` si la bascule a modifié la configuration (no-op quand le
  /// profil demandé était déjà l'intention persistée sans divergence).
  final bool modified;

  /// Au moins une clé à redémarrage a changé — la posture effective ne
  /// sera atteinte qu'au prochain démarrage du daemon.
  final bool restartRequired;
}
