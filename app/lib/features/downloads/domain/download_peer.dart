/// Pair BitTorrent connecté à un téléchargement — miroir du dict
/// `peers[]` de `GET /api/downloads?get_peers=1` (format Python).
class DownloadPeer {
  const DownloadPeer({
    required this.ip,
    required this.port,
    required this.extendedVersion,
    required this.direction,
    required this.downrate,
    required this.uprate,
    required this.dtotal,
    required this.utotal,
    required this.connectionType,
  });

  final String ip;
  final int port;

  /// Nom du client distant (`extended_version`).
  final String extendedVersion;

  /// `L` = connexion entrante (remote initiated), `R` = sortante.
  final String direction;
  final int downrate;
  final int uprate;

  /// Totaux échangés avec ce pair (`utotal`/`dtotal`).
  final int dtotal;
  final int utotal;

  /// Type de transport (`connection_type` : TCP/uTP selon rqbit).
  final String connectionType;
}
