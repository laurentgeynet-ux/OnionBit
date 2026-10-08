# ADR-0018 — Arborescence portable et zones de téléchargement public / privé

Statut : **Proposée** (2026-10-08) — suite logique des ADR-0015/0016/0017
(extensions, identité portable, transport furtif). ADR-0016 est
**implémentée** : graine HKDF, phrase BIP39, gate `pending`/`locked`/
invité et `OBSK` sont livrés — les fondations de la zone privée
existent déjà. Cadre de la Phase 11 de `docs/plans/roadmap.md`.

## Contexte

Deux constats convergents :

1. **L'arborescence n'est pas réellement portable.** Le `state_dir`
   mélange à plat tout l'état du nœud :

   ```
   state_dir/
     configuration.json     — api.key
     onionbit.db            — catalogue, downloads, messages, peers…
     identity_seed.bin      — graine racine (ou blob OBSK)
     ipv8_keypair.bin       — cache dérivé
     stealth_bridge.key     — cache dérivé
     downloads/             — téléchargements, en clair
     rqbit/{main,anon<N>}/  — fastresume session.json + <ih>.bitv
     logs/  .onionbit.lock  exitnodes.txt  _m_torrent_titles.*  web/ …
   ```

   Pire : des **chemins absolus sont persistés** —
   `downloads.output_dir`, `downloads.completed_dir`,
   `libtorrent/download_defaults/{saveas,completed_dir,torrent_folder}`,
   `watch_folder/directory`, `api/web_ui_dir`, `api/https_certfile`,
   `trackers_file`, et `output_folder` dans le `session.json` de
   librqbit. Déplacer le bundle ou changer de lettre de lecteur USB
   (`E:` → `F:`) casse les chemins stockés : les téléchargements
   restaurés ne retrouvent plus leurs fichiers. Un daemon « portable »
   (clé USB, disque externe, copie de dossier entre OS) n'est possible
   qu'en apparence.

2. **Aucune distinction sensible / non sensible.** Tout téléchargement
   atterrit en clair dans le même `downloads/`. Or l'identité portable
   (ADR-0016) fournit déjà une racine de clé capable de protéger une
   zone privée, et la convention de blob portable (`OBID`/`OBV1`/`OBSK`
   : magic + version + sel/nonce aléatoires + AEAD + borne de taille)
   est établie. Le seul morceau manquant est un **backend de stockage
   chiffré pour les données de torrents** — et librqbit vendored expose
   exactement le point d'insertion : les traits `StorageFactory` /
   `TorrentStorage` (`pread_exact`/`pwrite_all` positionnés, synchrones
   par design pour éviter les copies) et `AddTorrentOptions.
   storage_factory`, un override **par torrent** — la zone privée peut
   donc coexister avec la zone publique dans la même session moteur.

Restriction mobile assumée : Android/iOS restent un **pilotage à
distance** de l'API (étape 19 remplacée) — la portabilité « clé USB »
concerne le daemon desktop (Windows/Linux/macOS). Un bundle multi-OS
peut toutefois embarquer l'APK client.

## Décision

### 1. Racine portable et arborescence cible

```
<racine portable>/               — dossier du bundle, clé USB, disque externe
├─ OnionBit.portable             — marqueur de mode portable (fichier
│                                  versionné, présence = portable)
├─ windows/  linux/  macos/      — binaires + UI par plateforme
│                                  (android/ peut porter l'APK client)
├─ state/                        — état du nœud (le state_dir) : SENSIBLE
│  ├─ configuration.json         — api.key
│  ├─ identity/                  — identity_seed.bin|OBSK,
│  │                              ipv8_keypair.bin, stealth_bridge.key
│  ├─ onionbit.db (+ wal/shm)
│  ├─ rqbit/{main,anon<N>}/      — fastresume par moteur
│  ├─ logs/  cache/              — onionbit.log*, exitnodes.txt,
│  │                              _m_torrent_titles.* …
│  └─ web/  https.pem  …         — UI servie, cert auto-signé, versions
└─ data/                         — contenu téléchargé : les ZONES
   ├─ public/
   │  ├─ temp/                   — téléchargements en cours (clair)
   │  ├─ downloads/              — fichiers complétés (clair)
   │  └─ torrents/               — .torrent sauvegardés (torrent_folder)
   └─ private/
      ├─ temp/                   — blobs OBD en cours (chiffrés)
      └─ downloads/              — blobs OBD complétés (chiffrés)
```

- **Marqueur `OnionBit.portable`** : `resolve_state_dir` remonte les
  ancêtres de `<exe>` à sa recherche ; trouvé → `state/` = racine du
  marqueur, `data/` = sa voisine. C'est lui qui rend un bundle
  multi-OS possible : `windows/onionbit-daemon.exe` et
  `linux/onionbit-daemon` sur la même clé **partagent le même
  `state/`** (remontée d'un niveau). Sans marqueur : comportement
  actuel inchangé (`<exe>/state` en bundle, `.onionbit/` sinon) —
  seule l'arborescence *interne* de `state/` change.
- **Migration douce de l'identité** : les fichiers à plat
  (`identity_seed.bin`, `ipv8_keypair.bin`, `stealth_bridge.key`) sont
  **déplacés** dans `state/identity/` au premier boot sous la
  nouvelle structure — jamais copiés (pas de doublon de secret),
  déplacement atomique par rename quand même volume. Compat lecture :
  si un fichier existe aux deux endroits, `identity/` fait foi et le
  doublon plat est supprimé (cohérence « graine autoritaire »
  d'ADR-0016 étendue aux chemins).
- `downloads/` historique (sous `state_dir`) migre vers
  `data/public/downloads` — ou, si le dossier est volumineux/hors du
  state_dir, laissé en place et enregistré comme chemin absolu
  explicite (voir §2) : la migration ne déplace jamais de gros volumes
  sans confirmation.

### 2. Chemins portables — plus aucun chemin absolu persisté par défaut

- **Racines nommées** résolues à l'exécution : `@state/`, `@public/`,
  `@private/` (syntaxe `@racine/sous/chemin` ; `/` portable — jamais
  `\` ni lettre de lecteur dans un artefact persisté). La résolution
  refuse `..`, les séparateurs mixtes et tout débordement hors de la
  racine visée.
- **Migration des chemins stockés** : à l'ouverture de la config et de
  la base, tout chemin absolu **situé sous l'ancien `state_dir`** est
  réécrit en forme portable (`<old>/downloads` → `@public/downloads`…).
  Tout chemin absolu **hors** de l'ancien `state_dir` est conservé
  tel quel — choix utilisateur assumé — mais tracé `warn` « non
  portable » et signalé dans l'UI.
- Surface couverte : `downloads.output_dir`, `downloads.completed_dir`,
  `saveas`, `completed_dir`, `torrent_folder`, `watch_folder/directory`,
  `api/web_ui_dir`, `api/https_certfile`, `trackers_file`, et —
  audit obligatoire — `output_folder` du `session.json` rqbit (patch
  vendored si nécessaire pour stocker la forme portable).
- API : `saveas`/`move_storage`/`completed_dir` acceptent la syntaxe
  `@racine/` en plus des absolus ; `GET /api/files/browse` peut
  naviguer `@state`/`@public` mais **`@private` n'est jamais listé par
  walk du FS** — les noms physiques y sont opaques (§3) ; le listing
  privé vient du manifest `OBM` (§3), exposé via un endpoint dédié
  sous `api_key_auth`.

### 3. Deux zones : `public` (clair) et `private` (chiffré, lié à l'identité)

Par téléchargement, nouvel attribut `storage_area ∈ {public, private}`
(colonne `downloads`, `PATCH`, choix à l'ajout dans l'UI).

- **`public`** — stockage filesystem inchangé. `temp/` reçoit les
  écritures en cours ; à la complétion, le contenu est déplacé vers
  `downloads/` (`storage.move_on_completion`, défaut **on** sous le
  nouveau layout) — réutilise le pipeline `move_storage` existant
  (fichiers déclarés du torrent seuls, déplacement hors executor,
  re-add avec re-hash de reconnaissance). `temp`/`downloads` restent
  deux sous-dossiers de la même zone : même FS, rename possible.
  Côté privé, tranché en revue externe : **rename physique des
  `.obd`** (même volume, O(1)) — `temp/` ne s'encombre pas de fichiers
  finis et les deux zones gardent la même sémantique.
- **`private`** — `EncryptedStorageFactory`
  (`onionbit-bittorrent::storage_private`), implémentation native de
  `TorrentStorage` injectée via `AddTorrentOptions.storage_factory` :
  - chaque fichier du torrent devient
    `data/private/{temp,downloads}/<groupe>/<nom>.obd` ;
  - `<groupe>` = HMAC-SHA256 tronqué de l'infohash sous `K_names` —
    **l'infohash ne doit pas apparaître dans les noms** : un infohash
    identifiable identifie le contenu via DHT/swarm ;
  - `<nom>` = HMAC-SHA256 du chemin relatif interne au torrent sous
    `K_names` — les noms de fichiers réels (souvent plus révélateurs
    que le contenu) ne quittent jamais la zone en clair ;
  - dérivation : `K_store = HKDF(racine_identité,
    "onionbit/private-store/v1")`, `K_names = HKDF(K_store, "names")`,
    `K_file = HKDF(K_store, "file/"‖infohash‖"/"‖relpath)` — domaines
    séparés comme ADR-0016, clé liée au torrent *et* au chemin ;
  - `racine_identité` = la graine (install seedée) **ou** `crypt_sk`
    (identité legacy — la zone privée fonctionne aussi en legacy, la
    phrase BIP39 reste la voie de sauvegarde la plus propre) ;
  - `pending`/`locked` (gate ADR-0016 livré : `pending` = aucune
    identité résolue sous `--first-run-gate`, `locked` = at-rest
    OBSK) → la zone privée est **fermée** dans les deux cas : ajouts
    privés et listing privé → `409 identity_locked`, téléchargements
    privés persistés restaurés en pause avec drapeau `locked_area`
    (cohérent : pas de clé, pas de données) ;
  - **invité** → `K_store` éphémère en mémoire ; la zone privée du
    disque n'est pas montée (une autre clé rend les blobs inertes de
    toute façon) ; les écritures privées de la session vont sous
    `private/temp/.guest/`, **purgé à la fermeture** — cohérent avec
    « rien n'est conservé » (les `.obd` résiduels seraient du bruit
    inexploitable, mais la promesse est zéro artefact).

#### Format `OBD` — fichier chiffré par chunks, en-tête scellé

Convention des blobs portables (AGENTS.md : version + nonce + AEAD +
borne), avec une spécificité arrêtée en revue : **aucun marqueur
statique** — même discipline que le transport ADR-0017 (« pas de
constante filaire »). Le magic n'est pas écrit en clair : il vit dans
l'en-tête *chiffré*.

```text
fichier  :  HDR_SLOT(4 Kio) ‖ chunk_0 ‖ chunk_1 ‖ …
HDR_SLOT :  hdr_nonce(12) ‖ hdr_ct ‖ scan_nonce(12) ‖ scan_ct ‖ pad(0)
            — offsets fixes : positions des chunks constantes
hdr_ct   :  AEAD(K_file, hdr_nonce, "obd/hdr")〔"OBD" ‖ v(1)
            ‖ chunk_log2(1) ‖ file_id(16) ‖ plain_len(8)
            ‖ reserved ‖ pad(0)〕            — clair bourré à taille
                                             fixe → hdr_ct à longueur
                                             constante, zéro fuite de
                                             longueur, parsing univoque
scan_ct  :  AEAD(K_scan, scan_nonce, "obd/scan")〔infohash(20)
            ‖ relpath_len(2) ‖ relpath(var) ‖ pad(0)〕
            — clair bourré à taille fixe lui aussi
K_scan   :  HKDF(K_store, "scan")            — clé de balayage unique,
                                             indépendante de K_file
chunk_i  :  nonce_i(12) ‖ AEAD(K_file, nonce_i, aad_i)〔plain_i〕
aad_i    :  file_id ‖ i:u64be                (lie le chunk au fichier
                                              et à sa position)
nonce_i  :  **aléatoire à chaque écriture**, stocké en tête du chunk
```

**Pourquoi un second sceau `scan_ct`** (revue externe 2, circularité
débusquée) : `K_file = HKDF(K_store, "file/"‖infohash‖"/"‖relpath)`
exige déjà le couple `(infohash, relpath)` — un en-tête scellé sous
`K_file` qui contiendrait ce couple serait **illisible lors du
balayage de secours** : circularité. `scan_ct` la brise : scellé sous
`K_scan` (dérivée de la seule graine), il s'ouvre aveuglément sur
chaque `.obd` → extraction de `(infohash, relpath)` → dérivation de
`K_file` → ouverture de `hdr_ct`. Si `manifest.obm` *et* `.bak`
sont perdus, ce scan O(N) reconstruit les groupes et les noms ; le
torrent se rattache ensuite par magnet au swarm. Sans la graine,
`scan_ct` est indiscernable de bruit — zéro marqueur. `relpath_len
= 0` = secours absent (chemin > capacité du slot, documenté — le
format reste opérationnel via l'`OBM`).

**Nonce aléatoire par écriture, jamais dérivé de l'index** — point
d'audit bloquant de la revue externe : `pwrite_all` *réécrit* des
chunks existants (RMW des écritures non alignées, re-téléchargement
d'une pièce après échec de hash, écritures `only_files` en bordure).
Un nonce déterministe `f(fichier, index)` chiffrerait alors deux
clairs sous le même `(K_file, nonce)` → réutilisation de nonce
ChaCha20-Poly1305 (keystream récupérable + forge). Même cause pour
l'en-tête : `plain_len` croît à mesure que le fichier s'étend →
l'en-tête est réécrit avec un `hdr_nonce` aléatoire neuf à chaque
réécriture. Surcoût : 28 o/chunk (nonce + tag) sur 64 Kio — négligeable.
Le nonce de fichier unique disparaît : chaque segment porte le sien.

`K_file` est dérivé **par (torrent, fichier)** :
`K_file = HKDF(K_store, "file/"‖infohash‖"/"‖relpath)` — l'infohash
dans le domaine interdit qu'un même `relpath` partagé entre deux
torrents privés aboutisse à la même clé (interdiction du swap de
chunks inter-fichiers, renforcé par `file_id` dans l'AAD).

À l'ouverture : dérivation de `K_file` → ouverture AEAD de l'en-tête
(un seul decrypt, coût nul) → refus typé si magic/version absents —
l'erreur est identique quelle que soit la cause (mauvaise clé, fichier
corrompu, non-OBD), aucun oracle différencié. Sans la clé, un `.obd`
est **indiscernable d'octets aléatoires** sous un nom HMAC : pas de
signature statique exploitable (reste l'entropie — §4).

- `chunk_size` = **16 Kio** par défaut (`storage.private_chunk_kib`,
  borné 16–1024) : calibré sur la taille de bloc BitTorrent — les
  offsets fichier ne sont pas alignés sur les pièces, donc la RMW
  reste nécessaire, mais un chunk de 16 Kio borne l'amplification à
  ~2×16 Kio par bloc au lieu de ~64+ Kio, et soulage les petites
  écritures aléatoires des clés USB.
- **RMW sérialisée par chunk — exigence de correction de
  concurrence** (revue externe 2) : le verrou de librqbit est *par
  pièce* (`live/mod.rs`), or un chunk `OBD` chevauche deux pièces
  (fichiers non alignés) → deux `pwrite_all` concurrents atteignent
  le même chunk ; deux RMW entrelacées perdraient silencieusement un
  bloc → échec de hash, re-téléchargement en boucle. Idem en lecture :
  `check_piece`/envoi pair lisant un chunk pendant sa RMW saisit un
  ciphertext déchiré → tag invalide parasite. La factory pose des
  **verrous rayés `RwLock` par (fichier, index)** : `pread_exact` en
  lecture partagée (zéro contention entre lectures), `pwrite_all` en
  écriture exclusive pendant sa RMW — pas de lock global.
- Présence d'un chunk : slot d'écriture fixe
  `HDR_SLOT + i × (chunk_size+28)` ; un slot **entièrement nul** =
  chunk jamais écrit → zéros en lecture (un slot nul ne peut pas être
  un chunk valide, le tag ne vérifierait jamais). Slot non nul mais
  AEAD invalide (corruption, troncature) → `warn!` + zéros :
  l'autorité d'intégrité reste le hash de pièce BitTorrent, rqbit
  re-télécharge — la couche OBD détecte et signale, elle ne doit pas
  figer le téléchargement. `ensure_file_length` fixe la longueur
  *logique* dans l'en-tête. La cohérence après crash repose sur le
  fastresume rqbit (`.bitv`) exactement comme sur la zone publique.
- **Sparse : honnêteté FS** (revue externe 2) — le fichier ne croît
  que des chunks écrits, mais « creux non alloué » n'existe que sur
  ext4/APFS et NTFS **marqué** (`FSCTL_SET_SPARSE` — sans marquage,
  un `pwrite` lointain remplit physiquement sous Windows aussi) ;
  **FAT32/exFAT remplissent physiquement** : écrire à l'offset
  500 Mo alloue et zéroifie tout l'amont — taille pleine immédiate
  et latence d'extension. La sémantique « slot nul → zéros » reste
  exacte partout ; l'économie d'espace est un bonus réservé aux FS
  sparse-capables, jamais une promesse.
- Pas de versionnement anti-rollback par chunk en v1 (un attaquant qui
  réécrit le disque peut déjà supprimer les fichiers — la menace visée
  est la lecture, pas la réécriture fine ; le tag AEAD détecte toute
  modification à l'ouverture du chunk).

#### Point d'intégration vendored — skip, pas whitelist

`session_persistence/json.rs` `update_db` refuse aujourd'hui toute
factory non-filesystem (`is_type_id` → `bail!`). Mécanique exacte
vérifiée : l'erreur remonte à `Session::add_torrent` (`session.rs`
~1612) et **fait échouer l'ajout entier** — sans patch, un torrent
privé ne peut même pas être créé.

Patch minimal (~15 lignes, lignée ADR-0007) : `update_db` **saute**
(`Ok(())` + `debug!`) les factories non-filesystem au lieu de
`bail!`. C'est le bon geste, pas un contournement :

- `session.json` n'est pas notre autorité de restauration —
  `SessionPersistenceConfig::Json { restore: false }` est déjà posé,
  `restore_downloads` depuis `onionbit.db` fait foi ;
- le fastresume `.bitv` est indépendant : `store_initial_check`/`load`
  sont adressés par `TorrentIdOrHash::Hash` (`initializing.rs`),
  `to_hash(Hash)` ne consulte jamais l'entrée de session ;
- sauter l'entrée évite d'écrire `<ih>.torrent` et `output_folder`
  en clair — une fuite de métadonnées privées sinon ;
- et écarte le risque qu'un `restore:true` futur ré-ajoute un privé
  avec la factory filesystem — écriture en clair, fuite majeure ;
- `delete()` rendu idempotent (`warn` → `debug` sur id absent) : les
  privés n'y figurent jamais.

#### Fuite `.bitv` — infohash en clair sur disque

Point d'audit de la revue externe : `bitv_filename` =
`{info_hash:?}.bitv` (`json.rs`) — le fastresume d'un privé
porterait l'infohash réel, en contradiction directe avec le HMAC des
noms/DB. De plus `delete()`/`clear(Id)` ne peuvent pas résoudre un
privé (absent de `session.json` → `to_hash(Id)` échoue) : rqbit ne
supprimerait jamais le `.bitv` privé.

**Solution : `OpaqueBitVFactory`** — wrapper du trait `BitVFactory`
(3 méthodes, `bitv_factory.rs`) autour du store interne, sans patch
vendored : `load`/`store_initial_check`/`clear` reçoivent
`TorrentIdOrHash::Hash(h)` → si `h ∈ private_hashes` (set en mémoire
alimenté par le moteur depuis l'`OBM` et les ajouts), délègue avec
`Hash(HMAC20(K_names, h))` → le fichier écrit est `<hmac>.bitv`.
Contrainte vérifiée : le mapping est **par hash, pas uniforme** — en
`locked`, `K_names` n'existe pas et la zone publique doit fonctionner
(les privés ne sont pas dans la session tant que la zone est fermée :
pas de storage factory sans clé). Le moteur maintient le set : ajout
avant `add_torrent`, retrait + **suppression explicite du
`<hmac>.bitv`** sur `remove_data` (rqbit ne peut pas le faire — il
connaît le hash via l'`OBM`). Bascule public→privé : le moteur
supprime l'ancien `<ih>.bitv` en clair ; un opaque est recréé.
`to_hash(Id)` sur un privé reste inoffensif : seul call site =
`clear` du chemin fastresume-corrompu, en `warn!` (consigné).
`fastresume_sampled_check` est off — pas de relecture de pièces.

#### Catalogue privé — manifest `OBM`

La table `downloads` ne doit pas révéler les métadonnées d'un contenu
privé. Pour les lignes `storage_area = 'private'` :

- la clé de ligne est `HMAC(K_names, infohash)` — l'infohash brut
  n'apparaît pas en base (un infohash en clair identifie le contenu
  via DHT/swarm) ;
- `name`, `source_uri`, `torrent_data` et les chemins internes sont
  externalisés dans un **manifest chiffré** `data/private/manifest.obm`
  — blob de la famille `OB*` : `nonce ‖ AEAD(HKDF(K_store,
  "manifest"))〔"OBM" ‖ v ‖ catalogue JSON borné〕`, réécrit atomique
  (tmp+rename+fsync dir) à chaque mutation, borne de taille à
  l'ouverture ;
- **résilience du manifest** (revue externe 2) : SPOF assumé et
  amorti — la réécriture conserve la copie précédente en
  `manifest.obm.bak` (rotation tmp → `.bak` → courant) ; en double
  perte, le sceau `scan_ct` de chaque `.obd` livre `infohash‖relpath`
  sous `K_scan`, permettant de reconstruire le catalogue par balayage
  avec la seule graine (les métadonnées `.torrent` se rattachent via
  magnet) ;
- **orphelins** : au montage de la zone privée (post-unlock), scan
  de cohérence — fichiers `.obd`/`<hmac>.bitv` absents du manifest →
  rapport + purge proposée (un crash pendant création/suppression
  laisserait sinon du bruit anonyme irrécupérable) ;
- indisponible tant que la zone privée est fermée (`locked`) —
  cohérent : le catalogue *est* la donnée privée.

`public` reste en clair dans `onionbit.db` comme aujourd'hui.
Conséquence directe sur la restauration : `restore_downloads` ne peut
pas router une ligne privée par `row.infohash` (c'est le HMAC — rqbit
ajouterait un torrent fantôme à l'infohash-HMAC). Le routage privé
lit l'`OBM` (vrai infohash, destination, état paused) ; zone fermée →
la ligne est sautée, marquée `locked_area` en mémoire, reprise à
l'unlock. `locked_area` n'est **pas** une colonne persistée : état
runtime dérivé de `storage_area × état du gate` — rien à écrire,
rien à migrer.

`shared.options.output_folder` d'un privé est ignoré par la factory
(noms HMAC) ; `stream_file` et `move_storage` doivent passer par le
trait `TorrentStorage`, jamais par un chemin FS reconstitué depuis
`output_folder` — sinon lecture d'un fichier inexistant ou, pire,
écriture en clair (test dédié).

#### API

- `storage_area` exposé dans `GET /api/downloads[]`, accepté par
  `PUT /api/downloads` (`destination`) et `PATCH` (déplacement
  inter-zones = move + conversion, pipeline `move_storage`).
- `GET /api/downloads/{id}/stream` fonctionne en privé : lecture à
  travers le storage (déchiffrement à la volée vers le client
  loopback) — **jamais** de déchiffrement vers un fichier temporaire
  en clair.
- `DELETE …?remove_data` sur un privé supprime les `.obd` (effacement
  logique — sur flash pas de garantie anti-forensique, documenté).
- `GET /api/private` (sous `api_key_auth`) : listing de la zone privée
  depuis le manifest `OBM` (noms réels, fichiers, progression), état
  `locked|mounted|guest-ephemeral`, compteurs — indisponible en
  `locked` (409) ; `locked` couvre aussi `pending` (aucune racine
  de dérivation tant que le gate n'a pas résolu l'identité).

#### Config

```jsonc
{
  "storage": {
    "private_enabled": true,        // zone chiffrée disponible
    "default_area": "public",       // zone des nouveaux ajouts
    "private_chunk_kib": 64,        // taille de chunk OBD
    "move_on_completion": true      // temp -> downloads (les deux zones)
  }
}
```

Aucune valeur en dur : tailles de chunk, bornes, rétention invité →
struct de config (AGENTS.md).

### 4. Menace et limites assumées

| Menace | Mitigation | Limite honnête |
| :--- | :--- | :--- |
| Vol/saisie du média | zone privée = blobs AEAD liés à l'identité, en-tête scellé | aucune signature statique ; la **présence** d'une zone d'entropie reste observable — pas de déni plausible |
| FAT32/exFAT sans ACL | `identity.at_rest` proposé par bandeau UI quand la racine est détectée amovible/sans-ACL | non bloquant ; sans at-rest, `state/identity/` reste lisible par tout montage |
| Infohash/noms dans les fichiers | noms opaques HMAC + catalogue `OBM` chiffré | tailles et *nombre* de fichiers observables (padding non prévu v1) |
| `onionbit.db` | lignes privées réduites à `HMAC(infohash)` — métadonnées dans `OBM` | le catalogue public reste en clair (zone `state/` sensible par construction) |
| Écrits résiduels OS | pagefile, indexation, miniatures | hors périmètre logiciel ; la zone publique est indexée normalement |
| Invité | purge `private/temp/.guest/` | écrasement flash non garanti (effacement logique) |

- **Une seule racine de secret** : pas de mot de passe séparé pour la
  zone privée — la graine BIP39/`OBSK` couvre déjà tout ; un second
  secret serait un second point de lockout sans gain.
- **Corrélation volume/horaires** : la taille et les dates des `.obd`
  restent observables — même statut assumé que le volume réseau en
  ADR-0017.
- **Média amovible** : le daemon détecte une racine sur volume
  amovible ou FS sans ACL POSIX (`GetVolumeInformation`/
  `f_flags`/`statvfs` selon l'OS — borne : « lecture des flags du
  volume, jamais de règle d'écriture affaiblie ») → flag exposé à
  l'API (`/api/identity` : `storage_removable: true`) → bandeau UI
  proposant le scellement at-rest. Le bandeau ouvre un dialogue mot
  de passe appelant `POST /api/identity/at_rest` — la bascule exige
  le mot de passe dans les deux sens et est refusée via
  `/api/settings` (ADR-0016 livré : un flag `true` sans `OBSK`
  réel serait un faux sentiment de sécurité). Non bloquant par
  défaut : l'utilisateur garde le choix — le bandeau est le
  garde-fou, pas le verrou.
- **`noexec` sur média amovible** (revue externe 2, nuancé) :
  udisks2 et macOS montent en `exec` par défaut, mais des fstab
  durcis et certaines distributions posent `noexec` sur les volumes
  amovibles — les binaires du bundle ne démarrent alors pas depuis
  la clé. Documenté dans le guide nomade (remontage `exec`, ou copie
  locale des binaires : `state/`+`data/` restent sur la clé).

## Alternatives rejetées

- **Chiffrement disque OS** (BitLocker/FileVault/LUKS/VeraCrypt) :
  hors logiciel, non portable entre OS, exige une préparation du
  média — complément possible, jamais un mécanisme OnionBit.
- **Blob unique par torrent** (un gros `OBD` par téléchargement) :
  incompatible avec `pread`/`pwrite` positionnés et la reprise
  partielle — le chunking est la seule forme compatible avec le
  trait `TorrentStorage`.
- **Clé de zone indépendante de l'identité** (mot de passe dédié) :
  second secret à sauvegarder/verrouiller ; la racine ADR-0016
  couvre déjà le besoin — complexité sans gain.
- **SQLite chiffré (SQLCipher)** : sortirait de `rusqlite` standard,
  mélange les responsabilités — le besoin réel (métadonnées des
  privés) est couvert par le manifest `OBM`, plus ciblé ; la DB vit
  déjà dans `state/`, zone sensible par construction.
- **Padding des tailles** : alourdirait chaque fichier d'un quantum
  fixe pour un gain de métadonnée marginal ; tailles = métadonnée
  assumée (comme ADR-0017 pour le volume réseau). Option
  `private_size_pad_kib` envisageable si le modèle de menace évolue.

## Conséquences

- **Positif** : un seul dossier `OnionBit/` copiable sur clé USB/
  disque externe, utilisable tel quel sur Windows/Linux/macOS (un
  sous-dossier de binaires par OS partageant `state/` + `data/`) ;
  plus aucun chemin absolu persisté par défaut — la lettre de lecteur
  ou le point de montage n'a plus d'importance ; téléchargement
  « sensible » disponible sans outil externe, déverrouillé avec
  l'identité et couvert par la même phrase BIP39.
- **Négatif** : surface nouvelle de stockage (codec `OBD` + factory +
  patch vendored persistence) — tests hostiles et fuzz requis avant
  tout défaut activé ; coût CPU AEAD par chunk (négligeable face au
  débit tunnel, mesurable en direct public).
- **Interop** : aucun impact filaire — les zones sont une affaire de
  stockage local ; Tribler ne voit rien de différent.

## Tests attendus (Phase 11)

- chemins portables : aller-retour `@racine/`↔absolu, refus `..`,
  migration d'un `state_dir` legacy (chemins réécrits, contenu
  intact), bundle déplacé/renommé → redémarrage identique ;
- `OBD` : round-trip, mauvaise clé/tag, troncatures à toutes les
  bornes, bit-flip par chunk, écritures non alignées + lectures
  partielles, chunk absent → zéros, borne de taille — **et oracle
  « zéro constante »** : deux fichiers de même contenu sous deux
  identités n'ont aucun octet commun, aucun magic reconnaissable ;
- **réécriture** : écrire deux fois le même chunk avec deux clairs →
  nonces distincts dans le slot (oracle anti-réutilisation), lecture
  = dernier clair ; idem pour la réécriture d'en-tête (extension de
  `plain_len`) ;
- **concurrence** : `pwrite_all` parallèles visant le même chunk →
  aucune perte de bloc (le verrou par chunk sérialise la RMW),
  contenu = union des écritures ;
- **manifest** : corruption de `manifest.obm` → reprise sur `.bak` ;
  double perte → reconstruction du catalogue par balayage des
  en-têtes scellés `infohash‖relpath` ; orphelins `.obd`/`.bitv`
  rapportés au montage ;
- `.bitv` : aucun `<infohash>.bitv` sur disque pour un privé (oracle
  listing + strings), `<hmac>.bitv` présent et fonctionnel, supprimé
  par `remove_data`, bascule public→privé sans résidu en clair ;
- manifest `OBM` : ligne DB privée sans infohash/nom en clair (oracle
  strings), catalogue intact après restart, indisponible en `locked` ;
- factory : `pread`/`pwrite` positionnés vs référence filesystem sur
  un torrent réel loopback — hash SHA-256 du contenu identique après
  decrypt ; fastresume privé (restart → reprise sans re-télécharger)
  malgré l'absence d'entrée `session.json` (vérifier aussi que
  `<ih>.torrent` n'est jamais écrit pour un privé) ;
- `locked`/`pending` : ajout privé → `409 identity_locked`, listing
  privé fermé, zone publique fonctionnelle ; `guest` → purge
  `temp/.guest/` constatée au shutdown ;
- amovible : volume sans ACL → `storage_removable` vrai + bandeau
  (activation via `POST /api/identity/at_rest`, mot de passe) ;
- banc `bench_portable.ps1` : `state/`+`data/` copiés vers un second
  chemin (lettre/dossier différent) → API identique, aucun chemin
  absolu résiduel dans les artefacts persistés (oracle grep).
