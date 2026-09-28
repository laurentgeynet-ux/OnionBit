/// Fichier d'un téléchargement (`GET /api/downloads/{ih}/files`).
class DownloadFile {
  const DownloadFile({
    required this.index,
    required this.name,
    required this.size,
    required this.included,
    required this.progress,
  });

  final int index;
  final String name;
  final int size;
  final bool included;

  /// Fraction téléchargée `[0.0, 1.0]` — conforme au `progress` Python
  /// (`files_completion`), pas des octets.
  final double progress;

  double get fraction => progress.clamp(0.0, 1.0);
}
