/// Tracker annoncé d'un téléchargement (`trackers[]` de `DownloadInfo`).
class DownloadTracker {
  const DownloadTracker({
    required this.url,
    required this.status,
    required this.peers,
  });

  final String url;
  final String status;
  final int peers;
}
