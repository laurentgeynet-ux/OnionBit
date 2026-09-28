import 'diagnostic_models.dart';

/// Contrat du dépôt diagnostic (`/api/ipv8/*`, `/api/logging`).
abstract interface class DiagnosticRepository {
  Future<List<OverlayInfo>> overlays();
  Future<List<CircuitInfo>> circuits();
  Future<List<RelayInfo>> relays();
  Future<List<ExitInfo>> exits();
  Future<List<SwarmInfo>> swarms();
  Future<List<TunnelPeerInfo>> tunnelPeers();

  /// Journal du daemon — réponse texte brut (`/api/logging`).
  Future<String> logs({int maxLines = 200});

  /// Mode debug du journal (`GET /api/ipv8/asyncio/debug` →
  /// `enable`) : à `true`, le daemon journalise les événements de
  /// cellules (create/extend/created/destroy, e2e, sorties).
  Future<bool> debugEnabled();

  /// Bascule le mode debug (`PUT /api/ipv8/asyncio/debug`).
  Future<void> setDebug(bool enable);
}
