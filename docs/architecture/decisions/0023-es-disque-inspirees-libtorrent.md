# ADR-0023 — Couche d'E/S disque inspirée de libtorrent : lecture non-créatrice, hash en vol, « fichiers manquants »

Statut : Proposée (2026-10-10). Partiellement implantée —
décisions 1, 2, 3 et 4 livrées (`9bf1c4f` + commit suivant),
décisions 5-6 en option (`docs/plans/roadmap_adr0023.md` ; le statut
global passera à Acceptée après la mesure de l'étape 84).

## Contexte

Trois symptômes utilisateur ont motivé un audit comparatif du chemin
disque de `librqbit` vendored face à libtorrent (checkout local
`D:\Projet\libtorrent\libtorrent`, qui pilote Tribler et qBittorrent) :

- un téléchargement volumineux semblait **retenu en mémoire** plutôt
  qu'écrit au fur et à mesure (« comme si le fichier était
  téléchargé en mémoire et n'était pas écrit au fur et à mesure sur
  le disque ») ;
- la suppression manuelle d'un fichier terminé provoquait un
  **re-téléchargement silencieux au même endroit** au redémarrage,
  et un `pread` paresseux **recréait les fichiers absents** (stub à
  la taille déclarée, contenu nul) — via trois chemins distincts
  (`fs.rs`, `mmap.rs`, `PrivateStorage`) ;
- un cycle `remove`/`re-add` laissait un téléchargement supprimé
  **se re-ajouter** (résurrection), et le daemon ne pouvait pas
  quitter tant que l'UI gardait le flux SSE ouvert.

Constats de l'audit (détail : `docs/plans/roadmap_adr0023.md`
« État des lieux ») :

| Mécanisme | libtorrent | rqbit vendored (avant) |
| :--- | :--- | :--- |
| Lecture d'un fichier absent | `open()` sans `create` → erreur → pièce manquante | `create(true)` → **stub recréé** |
| Vérification d'une pièce téléchargée | hash calculé **depuis le cache d'écriture** (`try_hash_piece`, `hasher_cursor`) — zéro relecture | `check_piece` **relit la pièce entière** (`pread_exact` 64 Kio) juste après l'avoir écrite |
| Cache d'écriture | blocs 16 Kio → `disk_cache`, flush en 4 passes + watermarks 7/8→3/4 sur `max_queued_disk_bytes` (100 Mio) | aucun — `pwritev` par pièce à sa complétion |
| Descripteurs | `file_pool` LRU global (40) avec déduplication d'ouvertures concurrentes | un `OpenedFile` par fichier par torrent, gardé ouvert |
| Back-pressure réseau | peers gelés à `max_size`, réveillés via `on_disk()` | sémaphore `block_in_place` seulement |
| Fichier absent au check | pièces sautées, **re-télécharge en place** — pas d'état « manquant » natif (qBittorrent l'ajoute sur `fastresume_rejected`) | idem — re-téléchargement silencieux |
| `verify_resume_data` | confiance = existence + taille ≥ attendue, pas de mtime | `.bitv` de confiance sans aucune vérification des fichiers |
| Backend mmap | `memcpy` dans pages mappées, writeback kernel paresseux, `store_buffer` (dédup read-your-writes) mais **pas de cache d'écriture watermarké** | `mmap` crée + `set_len` même en lecture |

## Décision

### 1. Lecture non-créatrice sur tous les backends — **implanté** (`9bf1c4f`)

Alignement sur la sémantique libtorrent (`pread_storage` ouvre en
lecture sans `create` ; `posix_storage` ouvre `"rb"`) : un `pread`
sur un fichier absent retourne une erreur que `initial_check`
traduit en pièce manquante — il ne **matérialise** jamais le
fichier. L'écriture garde la création paresseuse ; un descripteur
ouvert en lecture est promu rw à la première écriture
(`ensure_open_mode`). Appliqué aux trois chemins :
`fs.rs` (`open_read` vs `ensure_open`), `mmap.rs` (`pread_exact`
délègue à `fs` — un mapping rw exigeait un fd writable), et
`PrivateStorage::materialize(false)` (un `.obd` absent est une
erreur de lecture, pas une création).

### 2. État « fichiers manquants » à la qBittorrent — **implanté** (`9bf1c4f`)

Ni libtorrent ni rqbit n'ont d'état natif pour « données déclarées
mais fichiers absents » : les deux re-téléchargent silencieusement
en place. qBittorrent ajoute `MissingFiles` sur rejet de fastresume.
On adopte ce modèle, adapté à notre layout `temp/`/`downloads/`
(ADR-0018) qu'aucun des deux n'a :

- **Détection** : ligne `finished` ou `total_downloaded > 0` dont
  les fichiers attendus (public : entrées `torrent_data` ;
  privé : décompte des `.obd` du groupe) manquent sous le
  `output_dir` persisté → set mémoire `Inner::missing` (hex
  infohash). Le garde `total_downloaded > 0` distingue
  « supprimé » de « jamais écrit » (fichiers paresseux d'un
  téléchargement neuf ne sont pas « manquants »).
- **Effet** : ré-ajout moteur **en pause** (visible dans
  `/api/downloads`, zéro écriture), stats patchées
  `state=Error` + `error="fichiers manquants"` → l'UI l'affiche
  comme qBittorrent (`STOPPED_ON_ERROR`).
- **Reprise explicite** (`resume_missing`) : `remove` + `re-add`
  systématique — le verdict du check pausé est périmé, un simple
  `unpause` ne relirait pas les fichiers (parité du `reload()` de
  qBittorrent). Contenu revenu → re-check en place ; toujours
  absent → `output_dir` rebasculé en `temp/`, re-add actif,
  `move_on_completion` ramènera en `downloads/` à la fin —
  **jamais de re-téléchargement silencieux dans `downloads/`**.
- **Persistance** : un `.bitv` de confiance peut rapporter
  `finished` sans relire le disque → la transition `finished` ne
  consomme pas la marque tant qu'il n'y a pas eu de vraie
  re-vérification.

### 3. Vérification depuis la pièce en RAM — **implanté**

`Piece<ByteBuf>` est en réalité **un seul bloc** (~16 Kio), pas la
pièce entière : le chemin historique faisait donc un `pwritev` par
chunk **puis** relisait la pièce complète (`check_piece`,
`pread_exact` 64 Kio) — pièce de 1 Mio ≈ 64 `pwrite` + 16 `pread`
≈ 80 appels disque. L'implémentation retenue va plus loin que le
hash seul, en reprenant le modèle libtorrent **staging → hash →
flush** :

- `TorrentStateLive::staged_pieces: Mutex<HashMap<u32, Vec<u8>>>`
  accumule les octets de chaque chunk (`stage_chunk`) — **aucune
  écriture disque** tant que la pièce n'est pas complète ;
- à `ChunkMarkingResult::Completed`, `FileOps::check_piece_data`
  hashe le buffer en RAM (comme `try_hash_piece` qui hashe depuis le
  `disk_cache`) — **zéro relecture** ;
- hash OK → `FileOps::write_piece` écrit la pièce d'un tenant (un
  `pwrite` par fichier touché, segments `attrs.padding` exclus) ;
- erreurs séparées : hash KO → `mark_piece_hash_failed` + coupure du
  pair (rien n'est écrit) ; écriture KO → `on_fatal_error`
  (sémantique inchangée) ; le compteur `downloaded_and_checked` et
  le bitfield ne bougent qu'après écriture réussie ;
- buffers libérés : pièce complétée, `PreviouslyCompleted` (chunk en
  double tardif), ou pièce requeueée à la mort d'un pair
  (`release_pieces_owned_by` retourne les pièces — signature
  étendue) ; borne mémoire ≈ pièces en vol × `piece_length` (une
  pièce par pair — le rôle du `disk_cache` 100 Mio de libtorrent).

`check_piece` (relecture disque) reste le chemin du **re-check**
(`initial_check`, `force_recheck`) où la donnée n'est pas en RAM.
Test `tests/staged_piece.rs` : hash RAM → `write_piece` →
`check_piece`/`initial_check` retrouvent tout, corruption refusée.

Conséquence assumée — identique à libtorrent : le hash couvre la
donnée transmise par le peer, pas ce qui a physiquement persisté ;
une corruption disque post-écriture n'est détectée qu'au re-check
suivant. Le cas `attrs.padding` de `file_ops` ne s'applique pas :
le buffer contient les octets envoyés par le peer (zéros de padding
inclus), le spécial-cas n'existe que pour le path disque où les
pad-files n'existent pas physiquement.

### 4. `allow_mmap` désactivé par défaut — **implanté**

Le backend mmap de libtorrent est cohérent **parce qu'il s'adosse**
à son file_view_pool + store_buffer + ticks de flush ; le backend
mmap vendored est minimaliste et repose sur le writeback paresseux
du kernel — exactement le symptôme « téléchargé en mémoire »
observé (pages dirty comptées comme cache, flush différé).
`allow_mmap: false` est restauré aux trois sites
(`EngineConfig::default`, `EngineConfig::offline`,
`LibtorrentConfig::default`) — la réversion locale non commitée de
`cc9ce37` était le seul diff de ces fichiers. Le backend reste
compilé et sélectionnable pour les bancs
(`libtorrent/allow_mmap`), avec ses lectures désormais
non-créatrices (décision 1).

### 5. Pool de descripteurs borné — **différé**

`file_pool` libtorrent (LRU global, borne `file_pool_size=40`,
déduplication des ouvertures concurrentes, destruction différée
hors mutex) est la réponse à « trop de descripteurs ouverts » sur
torrents multi-fichiers. Notre `OpenedFile` paresseux garde un fd
par fichier par torrent — borne = Σ fichiers. Non critique tant que
les torrents restent à cardinalité raisonnable ; étape dédiée si le
besoin se confirme (configuration `storage.file_pool_size`,
remplacement du `Vec<OpenedFile>` par un pool LRU partagé).

### 6. Hints de lecture à la vérification — **optionnel**

`async_hash` passe `sequential_access | volatile_read` →
`madvise(DONTNEED)` / `FILE_FLAG_SEQUENTIAL_SCAN` : la relecture de
check n'évince pas le cache OS des autres processus. Petit patch
dans `initial_check`/`update_hash_from_file` si la vérification de
gros volumes montre un impact.

### 7. Explicitement non adoptés

- **Fences au niveau job disque** (`disk_job_fence`) : la sérialisation
  remove/move/check existe déjà au bon niveau via `lifecycle_gate`
  (`3f6495e`) — porter des fences dans le vendored serait une
  réécriture du modèle de concurrence sans gain.
- **`part_file`** (priorité-0) : OnionBit n'expose pas de priorité
  par fichier.
- **Cache d'écriture watermarké complet** : rqbit écrit déjà par
  pièce entière coalescée (`pwritev`) ; le read-back supprimé par la
  décision 3 était le double I/O réel. Un `disk_cache` à la
  libtorrent n'apporterait que le lissage des petites écritures —
  surcoût d'architecture sans besoin mesuré.
- **`stat_cache`** : notre détection d'absence stat par fichier au
  restore n'a pas de hot-path qui la justifierait.

## Conséquences

- **Mémoire** : plus de rétention par writeback kernel en backend
  par défaut (décision 4) ; la pièce en RAM vit le temps
  d'assemble→écrire→hasher puis est libérée (inchangé vs actuel).
- **I/O** : la boucle de téléchargement fait `pwritev` + hash RAM —
  les lectures disque restantes sont le seeding, le streaming et
  les re-checks.
- **Fastresume** : reste l'autorité de progression persistée ; la
  confiance reste *existence + taille* (la nôtre est même plus
  stricte : taille exacte vs ≥ chez libtorrent). Un `.bitv` de
  confiance ne peut plus faire disparaître la marque « manquant »
  sans re-vérification réelle.
- **Privé** : les `.obd` suivent la même sémantique (lecture sans
  matérialisation) ; la détection « manquant » couvre la suppression
  partielle par décompte.
- **Interop/fidélité** : décisions 3-5 modifient le vendored —
  écart documenté au filaire : **nul** (toutes ces décisions sont
  internes au stockage, aucune trame n'en dépend). Le hash en RAM
  n'affaiblit pas le tit-for-tat : une pièce corrompée en RAM avant
  écriture échoue au hash pareil.
- **Limite connue** : sans fences job-level, une pièce en cours
  d'écriture lors d'un `delete_files` s'appuie sur `lifecycle_gate`
  + `PrivateStorage` `retired` — déjà couvert et testé.

## Références

- libtorrent `src/pread_disk_io.cpp` (`async_write` ~707-765,
  `try_flush_cache` ~1872, `do_job(check_fastresume)` 1432),
  `src/disk_cache.cpp` (`flush_to_disk` 920-1094, `try_hash_piece`
  381), `src/back_pressure.cpp` (watermarks 7/8→3/4),
  `src/file_pool_impl.cpp`, `src/storage_utils.cpp`
  (`verify_resume_data` 484, `initialize_storage` 671),
  `src/mmap_storage.cpp` (`write` 600, `tick` 977),
  `src/torrent.cpp` (`start_checking` 2586, `on_piece_hashed` 2666),
  `include/libtorrent/settings_pack.hpp` (`max_queued_disk_bytes`,
  `file_pool_size`, `checking_mem_usage`, `hashing_threads`).
- qBittorrent `torrentimpl.cpp` (`handleFastResumeRejected` →
  `m_hasMissingFiles`, `MissingFiles`, `prepareResumeData`
  conservant `have_pieces`).
- rqbit vendored : `src/torrent_state/live/mod.rs` (`write_to_disk`
  ~1856), `src/file_ops.rs` (`check_piece`, `update_hash_from_file`),
  `src/storage/filesystem/{fs,opened_file}.rs`,
  `src/storage/examples/mmap.rs`.
- Commits : `9bf1c4f` (décisions 1-2), `3f6495e` (lifecycle_gate),
  `46e4b77` (shutdown borné), `06ddf91` (décapsulation streamée).
- Plan : `docs/plans/roadmap_adr0023.md`.
