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
}
