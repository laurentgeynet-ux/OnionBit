# ADR-0008 — Stockage paresseux : erreurs d'E/S différées au premier accès

Statut : Acceptée (2026-09-30).

## Contexte

L'initialisation d'un torrent multi-fichiers appelait, pour chaque
fichier, `create_dir_all` + `CreateFile` + marquage sparse +
`set_len` — ~31 ms par fichier sous Windows, soit **349 s** pour un
torrent de 11 310 fichiers au démarrage du daemon (mesuré par
l'instrumentation `890ee72`, cf. `CHANGELOG.md`).

Le stockage vendored (`vendor/librqbit`) est devenu paresseux :
`OpenedFile::new_lazy` n'enregistre que le chemin et
`FilesystemStorage::init` / `MmapFilesystemStorage::init` ne font plus
aucun appel disque — les descripteurs et les mmaps sont créés à la
première lecture/écriture (`ensure_open`), comme le file pool de
libtorrent. Restauration ramenée à quelques millisecondes.

Conséquence directe : **les erreurs de chemin de sortie invalide ou de
permissions ne sont plus détectées à l'ajout du torrent, mais à la
première E/S** (fastresume échantillonné, `initial_check`, écriture de
pièce, streaming). De plus, le `fastresume_check` restant activé par
défaut, ses lectures échantillonnées peuvent se poursuivre après que les
torrents sont visibles dans l'API : « restauration terminée » et
« vérification terminée » sont deux états distincts.

## Décision

1. **Erreur différée assumée** : un `output_folder` impossible
   (parent inexistant non créable, fichier à la place d'un dossier,
   permissions manquantes) n'échoue plus dans `add_torrent`/`init`.
   L'erreur remonte au premier accès via `ensure_open` →
   `Error::Anyhow`/`FsFileIsNone`, déjà traitée par `initial_check`
   (pièces marquées manquantes, le check continue) — pas de donnée
   incorrecte, pas de crash, pas d'état `Error` fatale pour une simple
   indisponibilité de dossier.

2. **État de check exposé** : `TorrentStateInitializing::check_started`
   (positionné à l'entrée de `check()`, après le sémaphore
   `concurrent_init_limit`) est publié via `TorrentStats.checking`.
   `Download::stats` mappe ainsi `Initializing` non pausé vers
   `Checking` → `HASHCHECKING` (statut Tribler 2) quand la validation
   tourne réellement, `WAITING_FOR_HASHCHECK` (1) tant que le torrent
   attend dans la file d'init — parité avec les `DLSTATUS_*` Python.

3. **Écart vs Tribler documenté** : Tribler (libtorrent) détectait les
   problèmes de destination plus tôt (init eager). Le report au premier
   accès est le prix de la restauration quasi instantanée ; c'est le
   comportement du file pool libtorrent lui-même (ouverture à la
   demande).

## Conséquences

- `GET /api/downloads` peut afficher un download sain dont le premier
  accès disque échouera ensuite (pièces → manquantes). L'UI doit
  tolérer ce passage d'état tardif ; c'est le comportement cible.
- Un torrent restauré peut rester en `HASHCHECKING` alors que
  `restore_downloads` est terminé : les mesures de démarrage doivent
  distinguer `total_elapsed_ms` de la restauration et la fin réelle des
  checks.
- `MmapFilesystemStorage` : un fichier devenu illisible **après** un
  mapping réussi ne remonte pas d'erreur `FileOps` (faute d'accès
  mémoire). Couvert : l'échec à la matérialisation. Non couvert : la
  troncature post-mapping — limite connue, à traiter si le backend mmap
  est réactivé en production.
- Les tests vendored sont désormais exécutables :
  `cargo test --manifest-path vendor/librqbit/Cargo.toml` (crates
  `exclude` du workspace + `[patch.crates-io]` local + `resources/`
  rapatrié de rqbit).

## Références

- `vendor/librqbit/src/storage/filesystem/opened_file.rs` —
  `OpenedFile::new_lazy`, `ensure_open`, `pending_len`.
- `vendor/librqbit/src/storage/filesystem/fs.rs`,
  `src/storage/examples/mmap.rs` — init sans disque + tests d'erreur
  différée.
- `vendor/librqbit/src/torrent_state/initializing.rs` — `check_started`,
  `validate_fastresume` (échantillonnage).
- `crates/tribler-core/tests/lifecycle.rs` —
  `restauration_sortie_inaccessible_erreur_differee`.
- `docs/CHANGELOG.md` — entrées « Perf : stockage paresseux » et
  « Stabilisation : erreurs différées » (2026-09-30).
