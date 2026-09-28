/// Tracker annoncé d'un téléchargement (`TrackerStatusDict` Python :
/// `{url, peers, seeds, leeches, status}` — utilisé à la fois dans
/// `downloads[].trackers` et `GET /downloads/{ih}/trackers`).
class DownloadTracker {
  const DownloadTracker({
    required this.url,
    required this.status,
    required this.peers,
    required this.seeds,
    required this.leeches,
  });

  final String url;
  final String status;

  /// `-1` tant que le tracker n'a pas été scrapé.
  final int peers;
  final int seeds;
  final int leeches;
}
