# ADR-0027 — Explorateur privé et export en clair de la zone privée

Statut : Acceptée (2026-10-10).

## Contexte

La zone privée (ADR-0018) stocke le contenu téléchargé sous forme
chiffrée : fichiers `.obd` (chunks ChaCha20-Poly1305 à nonce
aléatoire + `K_file` par fichier), noms HMAC, catalogue dans
`manifest.obm` — tout est lié à l'identité et illisible sans elle.
C'est le comportement voulu.

Trois trous fonctionnels liés :

1. **Aucun chemin de lecture sans changer de zone** —
   `move_storage` privé → public déchiffre mais *déplace* (ligne
   redevient claire, ré-encapsulation au retour). Pour « juste
   lire/copier un fichier », c'est lourd et sémantiquement faux.
2. **Aucune visibilité du contenu hors `GET /api/downloads`** —
   le catalogue `manifest.obm` connaît des entrées sans ligne
   `downloads` (retrait de la liste, restauration différée) : le
   contenu existe, est déchiffrable, mais l'utilisateur ne sait
   pas qu'il l'a. Un explorateur ne s'appuie donc pas sur la
   liste de téléchargements mais sur le **manifeste** — qui liste
   noms réels, infohashes et `torrent_data` de chaque entrée.
3. **« Ouvrir le dossier » sans objet en zone privée** — corrigé
   côté UI (2026-10-10) : le spec `@private/…` passé à
   `explorer.exe` retombait sur `Documents`. L'action est masquée
   en privé ; l'explorateur la remplace fonctionnellement.

Brique déjà en place : `GET /api/private` sert le catalogue
déchiffré (`state`, `downloads[]` avec vrais noms, `orphans`) —
protégé par `api_key_auth` et réservé au titulaire (identité
déverrouillée = `state == "mounted"`).

## Décision

### 1. Explorateur privé : vue interne en clair sur le manifeste

Le titre du coffre est le manifeste — pas la liste de
téléchargements. Endpoints d'extension (même protection
`api_key_auth`, `409 identity_locked` quand la zone est
`locked`/`guest`) :

- `GET /api/private` — catalogue (existant : nom, infohash,
  sous-racine, `added_on`, orphelins).
- `GET /api/private/{key}/files` — fichiers d'une entrée :
  `key` = `row_key` opaque *ou* infohash réel (indifférent pour
  l'appelant, comme les DELETE/PATCH privés) ; liste
  `{index, path, length}` dérivée du `torrent_data` du manifeste
  (le `.torrent` est persisté — les noms sont connus même hors
  ligne et sans objet moteur). `404` si l'entrée est absente du
  manifeste.
- Entrées manifeste **sans `torrent_data`** (reconstruites par le
  scan de montage) : les noms sont récupérés par balayage du
  groupe — chaque `.obd` porte son `(infohash, relpath)` en clair
  dans le sceau `scan_ct` (`K_scan`, dérivé de l'identité) ; la
  taille vient du `plain_len` de l'en-tête `OBD`. Un sceau absent
  (`relpath_len = 0`, chemin > 3300 o) rend le `.obd`
  indéchiffrable — `K_file` exige le couple — : fichier omis.
- **Orphelins `OrphanReport`** (groupes dont le `scan_ct` a
  *échoué* au montage) : contenu **indéchiffrable** — sans
  `(infohash, relpath)`, `K_file` n'est pas dérivable ; ces
  groupes sont hors portée de l'explorateur (ils relèvent de
  `DELETE /api/private/orphans`, déjà existant — l'API ne les
  expose d'ailleurs que par compteurs).

UI : page « Zone privée » (accessible uniquement quand
`state == "mounted"`) — arborescence par entrée manifeste, taille
et date, orphelins distingués ; sélection de fichiers puis
export (§2) ; ouverture d'un fichier = export vers le cache local
puis `openPath`.

### 2. `export` : déchiffrement d'une *copie*, zone inchangée

`export_private(row_key, dest_dir, files?)` — analogue au chemin
`privé → public` de `move_across_zones`, mais **sans aucune
mutation** : ni `remove_engine_only`, ni `downloads::upsert`, ni
déplacement. L'entrée vise la **clé manifeste** (pas l'objet
moteur) — un contenu dont la ligne `downloads` a été retirée
reste exportable tant que le manifeste le connaît.

- Lecture via la couche de stockage privé (`K_file`, manifeste)
  — **pas** d'accès direct au `.obd` sur disque : pas de souci de
  partage de handles Windows ni de topologie opaque dans le code
  appelant.
- Noms de sortie = métadonnées du `torrent_data` (manifeste),
  sinon `relpath` retrouvé par `scan_ct` (entrée sans
  `torrent_data`), jamais les noms HMAC ; sceau absent →
  fichier non exportable (clé non dérivable), omis.
- `files: Option<Vec<usize>>` — export partiel ; `None` = tout.
- `dest_dir` résolu par `paths.resolve_input` (`@private/…`
  refusé — exporter dans la zone n'a pas de sens).
- Journalisation sans noms de fichiers (métadonnées sensibles) :
  compte de fichiers et d'octets seulement.
- Copie longue → `spawn_blocking`, bornée comme le move ; v1
  synchrone (pas de job de fond ni d'annulation, à revoir si
  l'usage le réclame).

### 3. REST

- `POST /api/private/{key}/export` — `{ "dest_dir": "…",
  "files": [0, 2] }` → `200 { "exported", "bytes", "dest_dir" }`.
  Erreurs : `404` clé inconnue du manifeste, `409`
  `identity_locked`, `400` `dest_dir` invalide. Sous
  `/api/private/*` (catalogue chiffré), pas `/api/downloads/*` —
  la cible n'est pas un téléchargement actif.

### 4. UI : explorateur avec menu contextuel par élément

Page « Zone privée » (visible seulement `state == "mounted"`) —
arborescence par entrée manifeste, tailles et dates, orphelins
distingués (non exportables — purge seule). **Clic droit** sur un
fichier ou une entrée :

- **« Lire »** — quand le type est affichable/lisible en
  local : export vers le cache temporaire de l'app
  (`getTemporaryDirectory`, nettoyé à la fermeture) puis
  `openPath` ; pour un aperçu interne (image, texte), lecture
  via le futur flux `content` (alternative §5) — sinon repli
  sur le même export-cache. Un fichier non affichable tombe
  sur « Extraire vers… ».
- **« Extraire vers un dossier… »** — dialogue de
  destination → `POST export` → toast (compte + octets,
  jamais de noms — convention §2).
- Entrée entière sélectionnée : mêmes actions sur tous ses
  fichiers ; multi-sélection supportée.

Le cache de lecture est marqué « copie non chiffrée » à
l'affichage comme l'export explicite. « Déplacer le dossier… »
reste le chemin de *migration* de zone.

### 5. Invariant de sécurité : pas d'identité, pas de lecture

Ce n'est pas un contrôle applicatif mais une propriété
cryptographique (ADR-0018) : `K_store = HKDF(racine identité)`
— graine `IdentitySeed` ou `crypt_sk` — et `K_file`/`K_manifest`/
`K_names` en dérivent tous (`obdfile.rs:237-308`,
`ZeroizeOnDrop`). Conséquences :

- **Identité supprimée ou remplacée** → `manifest.obm`
  indéchiffrable, `.obd` indéchiffrables : le contenu devient du
  bruit irrécupérable, *même avec l'accès disque complet*. Un
  attaquant qui « change l'identité » n'obtient rien — changer
  l'identité ne débloque pas les fichiers, ça les condamne.
- **Zone `locked`/`guest`** → les *nouveaux* endpoints
  (`files`, `export`) répondent `409 identity_locked` : aucune
  lecture de contenu, aucune exportation. `GET /api/private`
  conserve son comportement existant : `200` avec
  `state:"locked"` et tableaux vides (le manifeste ne peut pas
  être ouvert sans les clés — il n'y a rien à fuiter).
- La seule surface d'exposition est l'**identité déverrouillée en
  session + `api_key_auth`** — même frontière de confiance que
  tout le reste du daemon (l'explorateur n'aggrave pas le
  modèle : quiconque détient déjà l'API authentifiée peut lire
  `GET /api/private`). La mitigation serait un verrouillage
  d'identité en cours de session — **aucun endpoint de relock
  n'existe encore** (seuls `unlock`/`create`/`guest`/`at_rest`
  sont routés) ; le seul moyen de jeter `K_store` aujourd'hui est
  le redémarrage du daemon. Un `POST /api/identity/lock` est une
  évolution candidate indépendante de cette décision.
- `K_scan` (secours `OBM` perdu) dérive aussi de `K_store` —
  pas de porte dérobée : impossible sans l'identité non plus.

## Alternatives considérées

- **`move_storage` privé → public tel quel** : réponse actuelle,
  rejetée pour « lire » — muter la zone (cycle remove/re-add,
  ligne reclairée) pour ouvrir un fichier est un effet de bord
  inacceptable.
- **Explorateur basé sur `GET /api/downloads`** : rejeté — la
  liste moteur ne voit ni les entrées manifeste orphelines de
  ligne ni les téléchargements retirés ; le manifeste est la
  source de vérité du contenu privé.
- **Endpoint de lecture en continu** (`GET
  /api/private/{key}/files/{i}/content`, déchiffrement à la volée
  + `Range`) : supérieur pour l'aperçu/streaming — le `.obd` est
  chiffré par **chunks indépendants** (nonce aléatoire par chunk),
  donc un accès positionnel existe déjà (`ObdFile::read_range` —
  chaque chunk lisible isolément, un Range déchiffre les chunks
  recouverts en entier) ; plumbing Range non trivial, **reporté**
  comme évolution possible sans changement de modèle.
  Sans lui, « Lire » (§4) passe par l'export vers le cache
  temporaire — même coût disque, zéro API de plus.
- **Ouvrir le dossier opaque réel** : rejetée — montre des blobs
  illisibles et expose la topologie HMAC ; pire qu'inutile.
- **Vue FUSE/dokany** : hors sujet à ce stade.

## Conséquences

- **Positif** : visibilité de tout le contenu privé (manifeste,
  pas la liste de téléchargements) + lecture/export sans renier
  la zone ; réutilise la routine de déchiffrement
  `move_across_zones` (propriétaire unique, pas de crypto
  dupliquée) ; `GET /api/private` existant = socle déjà sécurisé.
- **Vigilance** : l'export produit une **copie en clair** — hors
  périmètre de la zone, responsabilité de l'utilisateur ;
  affiché dans l'UI (« copie non chiffrée »).
- **Limites assumées** : entrées sans `torrent_data` exportées
  sous les `relpath` retrouvés par `scan_ct` (noms réels
  récupérés — sceau absent → fichier omis, indéchiffrable ;
  groupes `OrphanReport` hors portée, indéchiffrables) ; v1
  synchrone, copie intégrale par fichier, pas de suivi de
  progression.
