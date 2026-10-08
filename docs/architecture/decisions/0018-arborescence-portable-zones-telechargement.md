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
    `K_file = HKDF(K_store, "file/"‖relpath)` — domaines séparés
    comme ADR-0016 ;
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
fichier  :  nonce(12) ‖ hdr_ct ‖ chunk_0 ‖ chunk_1 ‖ …
hdr_ct   :  AEAD(K_file, nonce)〔"OBD" ‖ v(1) ‖ chunk_log2(1)
            ‖ plain_len(8) ‖ reserved〕      — en-tête scellé
chunk i  :  AEAD(K_file, nonce_i, plain_i)   — ≤ chunk_size + tag(16)
nonce_i  :  nonce[0..8] ‖ i:u32be            (12 o — jamais réutilisé)
AAD_i    :  hdr_ct ‖ i                        (lie le chunk au fichier)
```

À l'ouverture : dérivation de `K_file` → ouverture AEAD de l'en-tête
(un seul decrypt, coût nul) → refus typé si magic/version absents —
l'erreur est identique quelle que soit la cause (mauvaise clé, fichier
corrompu, non-OBD), aucun oracle différencié. Sans la clé, un `.obd`
est **indiscernable d'octets aléatoires** sous un nom HMAC : pas de
signature statique exploitable (reste l'entropie — §4).

- `chunk_size` = 64 Kio par défaut (`storage.private_chunk_kib`,
  borné 16–1024) : `pread`/`pwrite` découpent sur les bornes de chunk,
  lecture-modification-réécriture des chunks partiels (les écritures
  torrent ne sont pas garanties alignées).
- Chunk jamais écrit → zéros en lecture : `ensure_file_length` fixe la
  longueur *logique* dans l'en-tête, le fichier physique ne croît que
  des chunks réellement écrits (sparse préservé — important sur clé
  USB). La cohérence après crash repose sur le fastresume rqbit
  (`.bitv`) exactement comme sur la zone publique.
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
  (tmp+rename) à chaque mutation, borne de taille à l'ouverture ;
- indisponible tant que la zone privée est fermée (`locked`) —
  cohérent : le catalogue *est* la donnée privée.

`public` reste en clair dans `onionbit.db` comme aujourd'hui.

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
