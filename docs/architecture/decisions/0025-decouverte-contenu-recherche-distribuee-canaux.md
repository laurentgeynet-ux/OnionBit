# ADR-0025 — Découverte de contenu : recherche distribuée et canaux curés

Statut : Proposée (2026-10-10). Plan d'implantation :
`docs/plans/roadmap_adr0025.md` (Phase 17, étapes 95-101).

## Contexte

OnionBit partage des fichiers (BitTorrent anonyme) mais ne permet
pas encore de les **trouver** — c'est le trou fonctionnel qui
transforme un transport en plateforme. Chez Tribler, la découverte
repose sur la « GigaChannel » : `MetadataCommunity` /
`RemoteQueryCommunity` (gossip de santé + requêtes SQL distantes)
alimentant la table `channel_node`, plus les **channels** :
collections signées de torrents dont la synchronisation passe par
des remote-select ciblés `channel_pk`/`origin_id`.

L'état réel du code est plus avancé que l'état perçu :

| Brique | État |
| :--- | :--- |
| `ContentDiscoveryCommunity` (gossip santé msg 3/4, `RemoteSelect`/`SelectResponse` msg 201/202, `VersionRequest`/`Response` msg 101/102) | **implémentée** dans `onionbit-ipv8::content_discovery` (~1200 lignes), côté requêteur **et** côté serveur (`remote_select` → chunks `send_db_results`) |
| Table `channel_node` + `torrent_state` + FTS5/`LIKE` de repli | **implémentée** (mapping Pony v15, requêtes bornées, pagination `max_rowid`) |
| `/api/search/remote` + `/api/metadata/search/local` + `/api/metadata/popular` + SSE `remote_query_results` | **implémentés** |
| Recherche augmentée (« slow search ») | **implémentée** (`onionbit-core::augmenter`, tokenizer maison) |
| `torrent_checker` (BEP-15 UDP + scrape) | **implémenté** |

Ce qui **manque** réellement :

1. **Validation interop** — le protocole de requête n'a jamais été
   exercé contre un nœud pyipv8/Tribler réel (extraction des
   métadonnées compressées LZ4 `.mdblob`, format exact du JSON de
   select, limites de paquets).
2. **Synchronisation des canaux** — rien n'abonne le nœud à un
   canal : pas de colonne `subscribed` en schéma (attribut Pony
   non persisté), pas de tâche de pull périodique des entrées
   `channel_pk`/`origin_id`, pas d'endpoints `/api/channels`
   dans le routeur. De plus, l'ingestion des réponses est
   **mémoire seule** par construction : `process_select_response`
   a été découplé de SQLite (la table accumulait ~26k entrées de
   gossip et ses scans figeaient la connexion partagée) —
   `seen_nodes`/`GossipMemory` remplacent la persistance pour la
   recherche éphémère.
3. **Émission de canal** — `encode_entry`/`encode_entry_presigned`
   signent déjà les mdblob ; le vrai trou est l'absence de
   logique de publication (racine de canal, commit, tombale) —
   après vérification filaire, la racine s'émet en
   `COLLECTION_NODE` (220), pas en `CHANNEL_NODE` (200 — aucune
   classe de payload Python, cf. §3).
4. **Mode `full`/stealth** — `enable_content_discovery && !stealth`
   (`ipv8_stack.rs`) : le réseau onionbit-only (ADR-0022 §7,
   ADR-0024 §8) n'a **aucune** découverte. C'est le seul maillon
   encore absent de la promesse « toile onion sans serveur »
   (ADR-0020).
5. **Robustesse serveur** — répondre à un select distant exécute du
   SQL paramétré : aucun rate-limit/budget par pair n'est codé en
   dur de manière centralisée.

## Décision

### 1. Le protocole filaire reste celui de Tribler — aucun nouveau message

`RemoteSelect`/`SelectResponse`, le gossip `Health` et le
`VersionRequest` sont déjà conformes aux IDs et au sérialiseur
pyipv8. Tout l'effort va dans (a) la validation interop et (b) ce
qui manque côté daemon — pas dans un nouveau format de paquet.
Écarts éventuels constatés en interop → `docs/reference_tribler/`
ou ADR dédié, jamais de copie verbatim du Python (AGENTS.md).

### 2. Abonnement aux canaux = pull périodique par remote-select

Modèle Tribler : « s'abonner » ne déclenche pas de gossip push —
le client interroge périodiquement un échantillon de pairs avec
`{channel_pk, origin_id}` et intègre les mdblob reçus.

- Migration `channel_node` : colonne `subscribed` (attribut Pony
  absent du schéma actuel) — marqueur persistant de l'abonnement ;
  la tâche `channel_sync` de `onionbit-core` émet, par canal
  abonné, un remote-select
  `{channel_pk, origin_id, metadata_type:[200,400], max_rowid}`
  vers `max_query_peers` pairs (réutilisation de
  `send_remote_select`, aucun nouveau type de trame). S'abonner à
  un `{channel_pk, origin_id}` jamais vu insère une **ligne
  racine placeholder** (`COMMITTED`, titre vide — le contenu
  arrive à la première sync), pas un 404.
- **Deux chemins d'ingestion distincts** (correction d'avis
  externe, état réel du code) :
  1. recherche éphémère (`search/remote`) → flux mémoire
     `seen_nodes`/`GossipMemory` + SSE, **inchangé** (la
     persistance gossip figeait SQLite) ;
  2. sync de canal (`channel_sync`) → ingestion **persistante**
     `channel_node` — même décompression LZ4 et même parse
     mdblob, puis `channel::insert`.
- **Anti-poisoning** (règle explicite) : la signature de chaque
  blob ingéré est vérifiée **et** comparée à la `channel_pk` de
  la requête — un pair ne peut pas injecter dans le canal
  d'autrui des entrées signées ailleurs (`verify_signature`
  existe dans `onionbit-format` mais n'est appelé sur aucun
  chemin d'ingestion aujourd'hui).
- **Bornes de persistance** : la fuite SQLite d'origine venait
  du volume — un plafond `channel_max_entries` par canal
  (config) + purge FIFO des entrées les plus anciennes hors
  `subscribed`/canal personnel empêche la reproduction du piège
  « 26k lignes » sur un canal populaire ou une sync hostile.
- **Ordonnancement** : round-robin avec jitter — un canal
  interrogé par fenêtre `channel_sync_interval` (config), jamais
  tous les canaux en rafale ; bornes identiques à la recherche
  (`first`/`last`, `packets_limit`).

### 3. Émission : canal personnel signé avec la clé primaire IPv8

Le nœud possède déjà une identité Ed25519 (`LibNaCLSK`, étape 2).
Le « canal personnel » = racine `COLLECTION_NODE` (type 220)
signée par cette clé + entrées `CHANNEL_TORRENT` (400).

> **Correction de fidélité (vérifiée sur `tribler/core/database/
> serialization.py`, Tribler 8.x)** : le type 200 `CHANNEL_NODE`
> n'a **aucune** classe de payload dans
> `METADATA_TYPE_TO_PAYLOAD_CLASS` (`ChannelNodePayload` n'est que
> la classe de base, jamais enregistrée) — un blob contenant du
> 200 lève `UnknownBlobTypeException` et tue le blob entier chez
> un pair Tribler. La racine de canal voyage donc en
> `CollectionNode` (220), que `onionbit-format` sait déjà
> sérialiser et signer ; le 200 reste `Rejected` en entrée et
> n'existe chez nous que comme marqueur de placeholder interne
> (jamais servi). Conséquence corollaire : `process_payload`
> Python ne persiste que `REGULAR_TORRENT` — les 220/400/500 sont
> parsés puis ignorés par Tribler ; les canaux curés sont une
> fonctionnalité **OnionBit-à-OnionBit**, compatible filaire mais
> sans réciprocité Tribler 8.x.

- `onionbit-format` : aucun travail — `CollectionNode`,
  `ChannelTorrent`, `Deleted` et `encode_entry`/
  `encode_entry_presigned` sont **déjà implémentés et testés**.
- `onionbit-core::channel_ops` : `commit` (copie l'info-hash en
  `CHANNEL_TORRENT` signé, `origin_id` = racine), `remove`
  (pierre tombale 500), `set_title` (racine 220 signée,
  renommage re-signé). Convention : la racine a
  `origin_id == id_` (cohérent avec l'abonnement `(pk, id)`).
- **Service des pierres tombales** : la colonne `signature` d'une
  ligne 500 porte la signature de l'entrée supprimée (=
  `delete_signature` filaire `DeletedMetadataPayload`), pas une
  signature de tombale — `remote_select` **re-signe à la volée**
  les 500 dont `public_key` est le nôtre (le pair distant vérifie
  `header.signature` ; une tombale non signée serait rejetée).
  Les tombales étrangères ne sont pas servies (non re-signables).
- **Réception `DELETED` (500)** : l'entrée passe `status`/
  `metadata_type` à `DELETED` (pierre tombale conservée —
  sinon une resync la réinsérerait) et `build_where` exclut
  `cn.status = DELETED` / `metadata_type = 500` des recherches
  locales. Note FTS5 : le trigger `fts_au` **ré-indexe** le
  titre à l'`UPDATE` — les tokens restent en `FtsIndex` mais
  la clause `status` les masque dans le `MATCH` ; purge
  complète seulement si la ligne est `DELETE` (alors `fts_ad`
  nettoie). Documenté, non bloquant.
- **Émission `DELETED`** : retirer une entrée de son canal
  personnel émet un blob 500 signé par la clé primaire
  (`encode_entry` supporte déjà le type) — les abonnés la
  retirent à la sync suivante. Endpoint dédié
  (`DELETE /api/channels/personal/{infohash}` ou équivalent
  Tribler).
- REST : `GET /api/channels`, `GET /api/channels/{pk}/{id}`,
  `PUT|DELETE /api/channels/{pk}/{id}/subscribe`, plus le canal
  personnel (extension — Tribler 8.x n'expose plus d'endpoint de
  publication) : `PUT /api/channels/personal` (création/nom de la
  racine), `PUT /api/channels/personal/{infohash}/commit`,
  `DELETE /api/channels/personal/{infohash}` — endpoints mutants
  couverts par `api_key_auth` comme tout `/api` (automatique,
  cf. `router.rs`).

### 4. Robustesse serveur : budget par pair sur les selects entrants

`remote_select` répondant n'importe quel JSON exécute du SQLite
paramétré — surface DoS **déjà présente** (le serveur select
répond aujourd'hui, sans attendre la phase 17). Le budget est
donc planifié **avant** tout travail de sync/émission (étape 96,
premier chantier après l'interop). Le provider impose : (a)
liste blanche des clés JSON (celles déjà gérées par
`build_where`), `DEPRECATED_SELECT_PARAMS` rejetés ; (b)
`first`/`last` bornés (défaut Python 1..50, maximum absolu
documenté) ; (c) fenêtre glissante par mid
(`ContentDiscoverySettings.max_select_per_peer` +
`select_window`, aucune valeur en dur) ; (d) `packets_limit`
existant bornant la réponse.

### 5. Découverte en mode `full` : remote-select **via sessions stealth**

Constat : `content_discovery` est court-circuité dès que stealth
est actif — justifié tant que la seule adresse atteignable est UDP
claire (le select révèle l'intérêt du requêteur au pair). Décision
(extension OnionBit-only, famille ADR-0015) :

- Les **ponts stealth** (ADR-0022 §7, rôle `bridge`/`gateway`)
  servent d'index relais : ils tournent `content_discovery` en
  clair côté IPv8 comme aujourd'hui ; le client `full` envoie son
  `RemoteSelect` **encapsulé** dans une trame `data` de sa session
  stealth vers un pont, qui l'exécute sur la community et renvoie
  les `SelectResponse` par le même lien.
- **Modèle de confiance — formulation honnête** (correction de
  revue) : le pont est l'*endpoint* de la session stealth — il
  voit **l'IP du client et son `client_id`** (la pk maîtresse
  est transportée dans `hs1` en v1) plus le JSON de requête.
  Ce que le pont *retient* au réseau, c'est l'origine : le pair
  legacy interrogé ne voit que l'IP du pont. La propriété réelle
  est **« anonymat vis-à-vis des pairs interrogés, confiance
  requise vis-à-vis du pont »** — modèle VPN-like, pas Tor-like.
  C'est déjà meilleur que Tribler (où le pair interrogé connaît
  l'expéditeur), mais à ne pas survendre : les abonnements
  `channel_sync` sont des intérêts *persistants* visibles du
  pont — pire qu'une requête ponctuelle.
- Format : extension payload `msg` OnionBit-only prévue par
  ADR-0015. **La trame `data` encapsulée est spécifiée
  génériquement** (type + payload opaque) — pas ad-hoc au
  `RemoteSelect` : c'est le véhicule prévu pour `VAULT_GET`/
  `MAILBOX_PULL` (pull store-and-forward, backlog P0) au lieu
  d'un mécanisme parallèle. Spec précise à l'étape 100.
- **Budget par session cliente** : le pont applique le budget
  de l'étape 96 **par session/identité cliente stealth**, pas
  par mid (un client furtif ne doit pas pouvoir faire exécuter
  au pont des selects au-delà de son propre quota).
- **Charge du pont** : `bridge` devient *index relais de
  contenu*, pas seulement porte d'entrée — les selects relayés
  coûtent du SQL côté pont. Nouvelle responsabilité du rôle,
  à dimensionner (quotas ci-dessus) ; réserver le service au
  rôle `gateway` reste une option de la spec si la charge sur
  les ponts d'entrée devient un problème.
- **Répartition inter-ponts** : avec plusieurs ponts
  *d'opérateurs distincts*, les selects sont distribués par
  canal/requête (rotation déterministe) — limite l'agrégation
  chez un opérateur unique. Limite honnête : les trois ponts
  actuels partagent le même VPS (ADR-0024 §8 — même IP/AS/
  opérateur) ; la rotation ne change rien tant que la diversité
  d'hébergeur n'existe pas.
- Le gossip de santé reste clair côté ponts ; le client `full`
  reçoit les santés dans les `SelectResponse` jointes (déjà le cas
  du format Python).

### 6. Santé interne : le checker alimente `torrent_state`, le select la sert

`torrent_checker` existe mais son périmètre de balayage est
limité aux téléchargements actifs. Extension : file périodique
`health_scan` couvrant les entrées `channel_node` non vérifiées
depuis `health_check_interval` (priorité `last_tracker_check`
croissant — Tribler `torrent_checker` fait pareil sur `popular`) ;
`HealthRequest` entrant répondu depuis `torrent_state` (déjà
implémenté, à confirmer en interop). Les réponses
`search/remote` joignent les santés `torrent_state` — comportement
`to_json` Pony existant.

## Non adoptés

- **DHT BitTorrent public pour les métadonnées** : contredit le
  modèle (la découverte reste dans l'overlay, pas dans le DHT
  global) et l'isolation `network-policy`.
- **Gossip push des mdblob complets** : Tribler a abandonné le
  push non sollicité des blobs ; le pull par select est borné,
  paginé et déjà câblé.
- **Nouveau protocole de requête maison** : le filaire Tribler est
  expressif (tri, filtre FTS, pagination, `channel_pk`) et
  l'interop est le but recherché.
- **Faire du canal un service distinct** : les canaux vivent dans
  `channel_node` + content-discovery — propriétaire unique
  (AGENTS.md), pas de crate `onionbit-channels`.

## Conséquences

- **Positif** : l'écosystème devient auto-suffisant — trouver,
  partager, curer sans index central ; interop Tribler pour le
  maillage legacy ; en mode `full`, les pairs interrogés ne
  voient plus que le pont relais (vs l'IP du requêteur en UDP
  direct chez Tribler).
- **Négatif / dette** : les ponts stealth agrègent les intérêts
  *et* connaissent le client (IP + `client_id` — modèle
  VPN-like, §5) ; la rotation n'aide qu'avec des ponts
  d'opérateurs distincts ; les selects relayés coûtent du SQL
  côté pont (budget par session) ; la tâche `channel_sync`
  ajoute du trafic de fond borné ; `subscribed` sans sync en
  mode `full` attend l'étape 100 — documenté à l'API.
- **Interop/filaire** : aucun écart nouveau côté IPv8/RemoteSelect ;
  l'encapsulation stealth de l'étape 100 est OnionBit-only
  (ADR-0015) et ne touche pas le mesh legacy.
