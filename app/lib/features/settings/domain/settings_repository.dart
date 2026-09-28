/// Contrat du dépôt réglages (`/api/settings`, `/api/shutdown`).
abstract interface class SettingsRepository {
  /// Arbre de configuration effectif du daemon (miroir `GET /api/settings`).
  Future<Map<String, dynamic>> get();

  /// Met à jour les réglages applicables à chaud (`POST /api/settings`).
  Future<void> update(Map<String, dynamic> settings);

  /// Demande l'arrêt du daemon (`PUT /api/shutdown`).
  Future<void> shutdown();
}

