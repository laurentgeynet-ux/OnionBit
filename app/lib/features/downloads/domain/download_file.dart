/// Fichier d'un téléchargement (`GET /api/downloads/{ih}/files`).
class DownloadFile {
  const DownloadFile({
    required this.index,
    required this.name,
    required this.size,
    required this.included,
    required this.priority,
    required this.progress,
  });

  final int index;
  final String name;
  final int size;
  final bool included;
  final int priority;

  /// Octets déjà téléchargés du fichier.
  final int progress;

  double get fraction => size <= 0 ? 0 : (progress / size).clamp(0.0, 1.0);
}
