import 'download.dart';
import 'download_file.dart';
import 'download_tracker.dart';

/// Opération de déplacement dans la file (`queue_position` Python).
enum QueueOp {
  up('queue_up'),
  top('queue_top'),
  down('queue_down'),
  bottom('queue_bottom');

  const QueueOp(this.wire);

  /// Valeur du paramètre `queue_position` du PATCH.
  final String wire;
}

/// Contrat du dépôt downloads (`GET/PUT/PATCH/DELETE /api/downloads`).
abstract interface class DownloadsRepository {
  Future<List<Download>> list();

  /// Ajoute un téléchargement (`uri` magnet/http(s) ou `torrent`
  /// chemin local). `anonHops > 0` exige `safeSeeding` (règle Python —
  /// l'UI la passe automatiquement quand l'utilisateur choisit un mode
  /// anonyme).
  Future<String> add({
    String? uri,
    String? torrentPath,
    String? destination,
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  });

  /// Ajoute un `.torrent` brut (`Content-Type: applications/x-bittorrent`,
  /// paramètres en query) — portable desktop et web (pas de chemin de
  /// fichier côté client).
  Future<String> addTorrentBytes(
    List<int> bytes, {
    String? destination,
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  });

  /// Fichiers du téléchargement (`GET /{ih}/files`).
  Future<List<DownloadFile>> files(String infohash);

  /// Trackers du téléchargement (`GET /{ih}/trackers`).
  Future<List<DownloadTracker>> trackers(String infohash);

  /// Ajoute un tracker à chaud (`PUT /{ih}/trackers`).
  Future<void> addTracker(String infohash, String url);

  /// Retire un tracker (`DELETE /{ih}/trackers` — persisté).
  Future<void> removeTracker(String infohash, String url);

  /// Ajoute les trackers de `download_defaults/trackers_file`
  /// (`PUT /{ih}/default_trackers`).
  Future<void> addDefaultTrackers(String infohash);

  /// Force une re-annonce aux trackers (`PUT /{ih}/tracker_force_announce`).
  Future<void> forceTrackerAnnounce(String infohash, String url);

  Future<void> pause(String infohash);
  Future<void> resume(String infohash);

  /// Change le nombre de sauts anonymes (`PATCH anon_hops`, seul
  /// paramètre autorisé par la requête — règle Python).
  Future<void> setAnonHops(String infohash, int hops);

  /// Déplacement dans la file (`PATCH queue_position`).
  Future<void> moveInQueue(String infohash, QueueOp op);

  /// Bascule la gestion automatique de file (`PATCH auto_managed`).
  Future<void> setAutoManaged(String infohash, bool enabled);

  /// Limites de débit individuelles en octets/s (`PATCH
  /// upload_limit`/`download_limit` ; `null` = ne pas toucher, `0` est
  /// ignoré par le backend — passer `null` pour désactiver via la
  /// valeur illimitée actuelle).
  Future<void> setRateLimits(
    String infohash, {
    int? uploadLimit,
    int? downloadLimit,
  });

  /// Ratio de seed individuel (`PATCH seeding_ratio`).
  Future<void> setSeedingRatio(String infohash, double ratio);

  /// Réinitialise le ratio au défaut (`PATCH seeding_ratio_default`).
  Future<void> resetSeedingRatio(String infohash);

  /// Re-vérification des données (`PATCH state=recheck`).
  Future<void> recheck(String infohash);

  /// Déplace les fichiers (`PATCH state=move_storage` + `dest_dir`,
  /// `completed_dir` optionnel).
  Future<void> moveStorage(
    String infohash, {
    required String destination,
    String? completedDir,
  });

  /// Sélection des fichiers à télécharger (`PATCH selected_files` —
  /// indices inclus ; liste vide = tous).
  Future<void> setSelectedFiles(String infohash, List<int> indices);

  /// Priorité d'un fichier (`PATCH file_priority=[index, 0..7]`).
  Future<void> setFilePriority(String infohash, int index, int priority);

  Future<void> remove(String infohash, {bool deleteFiles = false});
}

