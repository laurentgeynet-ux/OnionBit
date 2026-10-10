// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Profils d'anonymat globaux (ADR-0022) — valeurs sérialisées du
/// contrat `GET`/`PUT /api/privacy/profile`.
enum PrivacyProfileKind {
  /// Défauts historiques — interopérable Tribler.
  legacy,

  /// Posture maximale — transport furtif, OnionBit↔OnionBit seulement.
  full,

  /// Combinaison libre (ou dérivation quand une clé couverte diverge).
  custom;

  /// Parse la valeur sérialisée API (`legacy`|`full`|`custom`) ;
  /// inconnu → `custom` (dégradation sûre : aucun preset n'est
  /// affiché comme actif).
  static PrivacyProfileKind parse(String? raw) => switch (raw) {
    'legacy' => legacy,
    'full' => full,
    _ => custom,
  };

  /// Valeur attendue par `PUT /api/privacy/profile`.
  String get wire => name;
}

/// État exposé par `GET /api/privacy/profile` : intention persistée
/// (`stored`), profil dérivé de la configuration courante
/// (`effective`), clés divergentes et signaux de session.
class PrivacyProfileState {
  const PrivacyProfileState({
    required this.stored,
    required this.effective,
    required this.divergedKeys,
    required this.restartPending,
    required this.guest,
    required this.bridgesConfigured,
  });

  /// Désérialise la réponse `GET /api/privacy/profile`.
  factory PrivacyProfileState.fromJson(Map<String, dynamic> json) {
    final stealth = json['stealth'] as Map<String, dynamic>? ?? const {};
    return PrivacyProfileState(
      stored: PrivacyProfileKind.parse(json['stored'] as String?),
      effective: PrivacyProfileKind.parse(json['effective'] as String?),
      divergedKeys:
          (json['diverged_keys'] as List?)
              ?.whereType<String>()
              .toList(growable: false) ??
          const [],
      restartPending: json['restart_pending'] == true,
      guest: json['guest'] == true,
      bridgesConfigured:
          (stealth['bridges_configured'] as num?)?.toInt() ?? 0,
    );
  }

  /// Intention persistée dans `configuration.json`
  /// (`privacy.profile`).
  final PrivacyProfileKind stored;

  /// Profil dérivé des clés couvertes — `custom` dès qu'une clé
  /// diverge du preset stocké. C'est ce que le sélecteur affiche.
  final PrivacyProfileKind effective;

  /// Clés couvertes ayant quitté le preset `stored` (tooltip du badge
  /// « Personnalisé »).
  final List<String> divergedKeys;

  /// Une bascule a été persistée mais les clés à redémarrage ne sont
  /// pas encore actives (diff config persistée ≠ config au bind).
  final bool restartPending;

  /// Session invitée : lecture autorisée, bascule refusée côté
  /// serveur (`409 guest_session`) — l'UI verrouille le contrôle.
  final bool guest;

  /// Nombre de ponts stealth configurés — `full` exige ≥ 1 ;
  /// à `0`, le dialogue propose la saisie d'un lien d'invitation.
  final int bridgesConfigured;
}
