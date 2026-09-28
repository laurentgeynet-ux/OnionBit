import 'download.dart';
import 'download_file.dart';

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
    int anonHops = 0,
    bool safeSeeding = false,
    bool paused = false,
  });

  /// Fichiers du téléchargement (`GET /{ih}/files`).
  Future<List<DownloadFile>> files(String infohash);

  Future<void> pause(String infohash);
  Future<void> resume(String infohash);

  /// Change le nombre de sauts anonymes (`PATCH anon_hops`, seul
  /// paramètre autorisé par la requête — règle Python).
  Future<void> setAnonHops(String infohash, int hops);

  Future<void> remove(String infohash, {bool deleteFiles = false});
}
