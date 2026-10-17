# ADR-0031 — Chiffrement de la base locale : codec sqlite3mc (ChaCha20-Poly1305 par page)

Statut : **Proposée** (2026-10-18) — jalon de décision = étape 136
(spike link + cipher) avant tout code de produit.
Plan d'implantation : `docs/plans/roadmap_adr0031.md`.
Backlog P1 « Chiffrement DB locale » (`docs/plans/backlog_priorites.md`).

## Contexte

La base SQLite (`Database::open`, `rusqlite` bundled) est aujourd'hui
le **pire artefact du bundle** au repos : elle concentre tout ce que le
chiffrement des blobs portables (`OBID`/`OBV1`/`OBD`) ne couvre pas.

Contenu en clair constaté :

- **Contenu** : `msg_messages.body` (corps déchiffrés pour
  l'affichage), `msg_contacts.alias` (pseudonymes attribués —
  souvent des noms réels), `msg_conversations`/`msg_members`/
  `msg_delivery` (graphe social + horodatages), `msg_attachments`
  (noms de fichiers privés), colonne `plane` des messages (le plan
  de transport `ipv8`/`onion`/`mailbox` est lui-même une métadonnée
  sensible).
- **Corrélation inter-plans** : `peer_onions` relie identité
  OnionBit ↔ adresses `.onion` — précisément le lien que le
  threat-model ADR-0029 réserve à la signature opt-in
  `onionbit/onion-bind/v1`. Une saisie de la DB livre
  gratuitement ce qu'un observateur réseau paierait cher.
- **Activité** : `downloads` (magnets, `output_dir`, `anon_hops`),
  `channel_node` (titres/tags des canaux suivis = centres
  d'intérêt), `rss_items`, `peer_stats`/`guards`/`ipv8_peers`/
  `tunnel_pex` (empreinte topologique), `attestations` (arêtes de
  la web-of-trust). `pull_store` transporte des blobs déjà chiffrés,
  mais `slot`/`kind`/expiry fuient présence et volume des messages
  en attente.

Cible de protection : **at rest** — saisie d'appareil, support
amovible perdu, sauvegarde copiée. Hors scope assumé : processus
vivant (mémoire, page cache OS), journal temporaire attaqué à chaud,
adversaire pouvant lire la clef en RAM.

## Options écartées

### VFS chiffrant custom (`sqlite-vfs`)

Techniquement faisable en safe Rust (le crate fait le FFI,
`sqlite3_vfs_register` auto-déclaré, wal-index émulé en mémoire
par régions de 32 Kio), mais réimplémenter la couche où vivent les
invariants de durabilité (ordre `xLock`/`xSync`, fichiers
`-wal`/`-shm`) est le chemin le **plus risqué** des trois : un bug
de verrouillage VFS = corruption silencieuse de la base du bundle,
et ce n'est pas détectable par la suite de tests usuelle. Rejeté.

### Chiffrement par colonne (AEAD applicatif)

Chiffrer `body`, alias, `peer_onions`, titres au niveau `rusqlite` :
simple, sans unsafe, mais le schéma, les index, les compteurs,
horodatages, infohashes et toutes les tables « publiques » restent
en clair — pour une app d'anonymat, la métadonnée *est* le contenu.
Ajoute aussi une liste de colonnes sensibles à maintenir qui
oubliera la prochaine table. Rejeté comme protection principale
(reste possible en défense en profondeur plus tard).

### SQLCipher (bundled ou provider custom)

- `bundled-sqlcipher-vendored-openssl` : exige `openssl-src` →
  NASM + perl — absent sous MSVC de la machine de build, chaîne
  fragile pour un binaire portable. Rejeté tel quel.
- `SQLCIPHER_CRYPTO_CUSTOM` : l'amalgamation accepte des callbacks
  `sqlcipher_provider_*` (HMAC-SHA256/512, AES-256-CBC, PBKDF2,
  random) câblés sur nos crates crypto. Fonctionne sans OpenSSL,
  mais impose ~300-500 lignes de shim C à écrire et auditer, et
  exige de patcher la build `libsqlite3-sys`. Reste l'option de
  repli si sqlite3mc échoue au spike.
- Nuance honnête : SQLCipher n'est pas « plus faible » en crypto —
  il authentifie chaque page (CBC + HMAC encrypt-then-MAC). La
  différence décisive est la **chaîne de build** et le format
  (AEAD par page chez sqlite3mc vs CBC+HMAC), pas la sécurité.

## Décision

### 1. Codec sqlite3mc, cipher `chacha20`

SQLite3MultipleCiphers (héritier wxSQLite3/sqleet, maintenu par
utelle) : amalgamation autonome = SQLite complet + codec + crypto
embarquée (ChaCha20-Poly1305 **AEAD par page**, AES-256-CBC,
Argon2 — implémentations dérivées libsodium, aucune dépendance
externe, pas de NASM/OpenSSL/perl). Licence MIT — GPL-compatible.

Pourquoi le codec et pas le VFS : le codec chiffre **après** la
couche pagination et **avant** le VFS natif — le VFS officiel
(verrouillage, `-wal`, `-shm`, synchro) reste intact et éprouvé.
Le WAL est chiffré automatiquement (les frames passent par le
pager) ; le wal-index (`-shm`) ne porte que checksums et numéros
de pages — métadonnée non sensible admise.

Intégration :

- L'amalgamation est compilée par un petit crate `-sys` maison via
  `cc` (build MSVC natif). Source : crate `sqlite3mc-src`
  (`205.1.0+sqlite3mc-2.5.1-sqlite-3.53.4` — bytes vérifiés par
  checksum Sigstore) **pinné en version exacte**, *ou* vendoring
  direct de la release GitHub sous `vendor/` (précédent
  `vendor/librqbit`). Arbitré au spike 136 : le vendoring supprime
  un intermédiaire fin mais ajoute ~10 Mio de source — le crate
  pinné suffit si la chaîne de supply est satisfaisante.
- `rusqlite` perd le feature `bundled` ; `libsqlite3-sys` linke
  notre `sqlite3.lib` via `rustc-link-search` émis par le build.rs
  du shim (bindings pré-générés : sqlite3mc est un superset de
  l'ABI sqlite3).
- Une seule instance SQLite dans le binaire — tout autre chemin
  d'ouverture passe par la même bibliothèque.

### 2. Clé : HKDF de la racine d'identité

`db_key = HKDF-SHA256(store_root, "onionbit/db-key/v1")` (32 octets),
même convention que `tor-hs/v1` et `tor-hs-vault/v1` (ADR-0029) :
domaine dédié, jamais la clef de signature, jamais le blob OBD.
Passée en **clé brute** au codec :

```
PRAGMA cipher = 'chacha20';
PRAGMA key = "x'<hex(db_key)>'";
```

Pas de passphrase/PBKDF2 embarqué à traverser : notre HKDF est déjà
le bon niveau de dérivation, la clef n'existe qu'en mémoire
(`zeroize` déjà dans l'arbre — `DbKey` droppée à la fermeture de
session).

Cycle de vie : `try_start_identity` résout `store_root` **avant**
`Database::open` — la clef est disponible au point d'ouverture
existant, aucun pré-chargement nécessaire. Session invitée : base
`:memory:` inchangée (rien à chiffrer). `Database::memory()` des
tests inchangée.

### 3. Migration des bases existantes

Détection au premier `open` :

- fichier absent/vide → création chiffrée directe ;
- 16 premiers octets == `SQLite format 3\0` → base **legacy claire**
  → migration ;
- sinon → base chiffrée → ouverture avec clef (échec typé
  `DbEncrypted` si la clef ne correspond pas — jamais « corrupt »).

Migration : `ATTACH` d'un fichier neuf chiffré + copie
(`sqlite3mc_export`/`VACUUM INTO` selon ce que valide le spike —
sqlite3mc accepte les paramètres URI `?cipher=&key=`), checkpoint
WAL avant, fichier produit à côté puis **swap atomique**
(`.plain.bak` conservé jusqu'à réouverture vérifiée, puis supprimé
— effacement best-effort documenté : pas de secure-delete garanti
sur FS grand public). Toute interruption laisse l'ancienne base
intacte (reprise idempotente).

Comportement bonus exploité : sqlite3mc **sans** `PRAGMA key` lit
une base claire comme un SQLite normal — la détection est donc un
simple sondage, pas un mécanisme dédié.

### 4. Configuration

`db.encrypted` (défaut `true`) — opt-out documenté pour
interop/diagnostic ; jamais de sélection automatique silencieuse :
une base existante en clair n'est migrée qu'avec le flag actif, une
base chiffrée n'est jamais rétrogradée en clair par ce mécanisme.

### 5. Conséquences threat-model (à reporter dans
`docs/security/threat_model.md`)

Couvert : saisie at-rest de tout le contenu DB (graphe social,
historique, ponts d'identité inter-plans). Limites assumées et à
documenter :

- les fichiers `-wal`/`-shm` voisins ne contiennent que des pages
  chiffrées et du wal-index — acceptable ;
- taille du fichier et cadence de croissance restent observables ;
- `mmap_size` est bypassé sur pages chiffrées (pragma conservé,
  no-op documenté) ;
- la clef vit en RAM pendant la session — l'hibernation/core-dump
  d'un processus vivant n'est pas couverte (rejoint la limite
  existante « at_rest » de l'identité).

## Questions arbitrées

1. **Codec** : sqlite3mc `chacha20` (AEAD/page). SQLCipher-custom
   en repli documenté.
2. **Clef** : HKDF `onionbit/db-key/v1` sur `store_root`, brute,
   zéro persistance, `zeroize` au drop.
3. **Défaut** : chiffrement activé ; opt-out explicite.
4. **Migration** : détection par magic, `ATTACH`+export, swap
   atomique, `.bak` effacé après réouverture vérifiée.
5. **Invité** : `:memory:` — hors scope.
6. **VFS custom** : rejeté (risque de corruption > gain).

## Jalons

Voir `docs/plans/roadmap_adr0031.md` — étape 136 = spike link +
cipher (GO/NO-GO avant code de produit), 137 = plomberie clef +
`db.encrypted`, 138 = migration legacy + swap, 139 = durcissement
(suite db complète chiffrée, crash/rollback, erreurs typées),
140 = docs + revue + clôture.
