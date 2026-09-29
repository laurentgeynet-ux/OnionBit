/// Aperçu d'un `.torrent` avant ajout (`PUT /api/torrentinfo/file`).
///
/// Les champs `trackers` et `isPrivate` sont une extension par rapport
/// à la réponse Python : ils permettent d'anticiper les annonces
/// impossibles via les tunnels anonymes.
class TorrentPreview {
  const TorrentPreview({
    required this.name,
    required this.trackers,
    required this.isPrivate,
  });

  final String name;
  final List<String> trackers;
  final bool isPrivate;

  /// Tous les trackers connus sont en HTTPS — injoignables via les
  /// sorties anonymes, qui ne relaient que du HTTP en clair one-shot
  /// (`http-request`/`http-response`). En mode anonyme, aucun pair ne
  /// sera découvert par ce biais.
  bool get httpsOnlyTrackers =>
      trackers.isNotEmpty &&
      trackers.every((t) => t.toLowerCase().startsWith('https://'));
}
