# Changelog — étapes franchies

Format : une entrée par étape de `docs/plans/roadmap.md`, la plus récente
en haut.

## Étape 9 — `tribler-ipv8` : overlay minimal (2026-09-27)

- `serializer.rs` : packers pyipv8 (`B/H/I/Q`, `varlenH`, `varlenHx20`,
  `ipv4` `>4sH`, `ip_address` type+donnees, `bits` MSB-first, `raw`,
  `20s/…`) — big-endian `struct`, bornes par `Reader`/`Writer`.
- `packet.rs` : paquet signe au format filaire exact — prefixe `0x00 +
  0x02 + community_id(20o)`, `msg_id`, `varlenH(pubkey)`, `Q(global_time)`,
  payload, signature Ed25519 64o couvrant tout ; verification a la
  reception avec rejet de prefixe etranger / signature invalide.
- `address.rs` : `UdpAddress` (IPv4/IPv6/domaine) = `UDPv4Address`/
  `UDPv6Address`/`DomainAddress` Python.
- `peer.rs` : `Peer` (cle publique + MID SHA-1) et `Network` (index par
  cle/adresse, `peers_for_service`).
- `endpoint.rs` : `UdpEndpoint` (socket UDP, dispatch par prefixe de
  22 octets comme `add_prefix_listener`).
- `discovery.rs` : `DiscoveryCommunity` (`community_id` Python inchange),
  msg 1-4 (similarity, ping/pong) et 245/246 (introduction ancien format),
  marche aleatoire `step()` (similarity-request vers pair connu, sinon
  introduction-request vers bootstrap).
- `LibNaClSecretKey` gagne `Clone` (necessaire aux reponses spawn).
- 4 tests : signature aller-retour, rejet signature corrompue, rejet
  prefixe etranger, et **decouverte loopback reelle** (deux noeuds UDP
  127.0.0.1 s'enregistrent mutuellement comme pairs verifies).
- Jalon d'interop avec un noeud pyipv8 reel : formats repris a
  l'identique ; test manuel reseau reporte (hors tests offline).

## Étape 8 — `tribler-daemon` : executable bout en bout (2026-09-28)

- Assemblage complet : `CoreSession` (moteur BitTorrent + SQLite +
  notifier) derriere `tribler-api`, servi par axum avec
  `with_graceful_shutdown` (Ctrl-C -> `session.stop()` -> arret
  propre).
- CLI clap : `--listen` (defaut `127.0.0.1:8085` = `DEFAULT_API` de
  tribler-cli), `--state-dir`, `--offline`. **Garde-fou** : refuse
  toute adresse d'ecoute non-loopback (l'API de controle n'est jamais
  exposee sur le reseau).
- Logging `tracing` fmt + `EnvFilter` (`RUST_LOG`, defaut info) —
  cf. regles de niveaux AGENTS.md (infos de cycle de vie seulement).
- 1 test e2e : binaire reel en `--offline`, poll de `/api/downloads`
  sur loopback, kill propre. Le test manuel "telechargement torrent
  reel" reste a l'etape 16 (necessite du reseau).

## Étape 7 — `tribler-cli` : CLI de pilotage (2026-09-28)

- Sous-commandes clap : `status`, `list`, `add` (`--paused`),
  `remove` (`--remove-data`), `pause`, `resume` ; option globale
  `--api` (defaut `http://127.0.0.1:8085`, constante `DEFAULT_API` —
  le Python utilise `api/http_port=0` aleatoire, on documente un port
  fixe pour le daemon de l'etape 8).
- `add` choisit le champ JSON selon la source (`uri` pour magnet/http,
  `torrent` pour un chemin local), comme l'API Python.
- Erreurs d'API affichees au format Tribler (`HTTP <code> : <message>`
  sur stderr, exit code 1).
- 1 test d'integration reel : binaire `tribler-cli` (via
  `CARGO_BIN_EXE_*`) contre un serveur `tribler-api` sur
  `127.0.0.1:0` — cycle complet + cas daemon injoignable.
- Piege corrige : `std::process::Command` bloque le runtime tokio
  mono-thread du test ; `tokio::process::Command` utilise.

## Étape 6 — `tribler-api` : REST + SSE (2026-09-28)

- Routeur axum (`router.rs`) + etat partage `AppState` (`CoreSession`).
- Endpoints : `GET /api/downloads` (filtres `infohash`/`excluded`),
  `PUT /api/downloads` (`uri` magnet/http ou `torrent` chemin local,
  `anon_hops` refuse tant que les tunnels ne sont pas faits),
  `DELETE /api/downloads/{infohash}` (`remove_data`),
  `PATCH /api/downloads/{infohash}` (`state` = `resume`/`stop`).
- `GET /api/events` : flux **SSE** au format exact du Python
  (`event: <topic>\ndata: <json>\n\n`), message initial
  `events_start`, topics mappés depuis `Notification` — correction de
  fidelite : le Python utilise SSE, pas WebSocket.
- `dto.rs` : `DownloadInfo` miroir du dict `info` Python (codes
  `DownloadStatus` 0..11 conserves, champs non encore disponibles emis
  avec les valeurs par defaut Python pour compatibilite clients).
- `error.rs` : `ApiError` → `{"error": {"handled", "message"}}`
  (identique a `rest_manager.py`), mapping `CoreError::Bt(NotFound)` →
  404.
- `docs/reference_tribler/api_rest_mapping.md` cree (endpoints, DTO,
  topics SSE, endpoints Python non couverts).
- 6 tests : 5 integration HTTP loopback (`127.0.0.1:0`, reqwest —
  cycle complet ajout/pause/resume/suppression, 400/404 au format
  Tribler, format SSE verifie) + smoke test du routeur.

## Étape 5 — `tribler-core` : Session + Notifier (2026-09-28)

- `CoreSession` : facade domaine (equivalent de `tribler.core.session.
  Session`) assemblant `BtEngine` + `Database` + `Notifier`.
- Restauration des telechargements persistes (`downloads`) au
  demarrage, avec re-pause si necessaire.
- Boucle de progression periodique (`progress_interval_ms`) :
  `DownloadProgress` par torrent + detection de fin
  (`DownloadFinished`) + marquage `finished` en base.
- `Notifier` : `tokio::sync::broadcast` borne (256), non-bloquant,
  variantes `SessionStarted/SessionStopping/DownloadProgress/
  DownloadFinished/DownloadStateChanged/TorrentMetadataCreated`.
- `CoreConfig` : tous les reglages d'orchestration (state_dir,
  downloads_dir, db_filename, intervalle, EngineConfig) + preset
  `offline` pour tests.
- `add_torrent_bytes` persiste les octets `.torrent` en base pour la
  reprise exacte au redemarrage.
- 2 tests : session offline bout en bout (ajout + stats + notification)
  et notifier sans abonnes.

## Étape 4 — `tribler-db` : schema SQLite + migrations (2026-09-28)

- `Database` : ouverture fichier/memoire, `Mutex<Connection>` pour le
  partage entre services async, WAL + `foreign_keys` actives.
- `migrations.rs` : migrations versionnees via `PRAGMA user_version`
  (`SCHEMA_VERSION = 1`), rejet des bases plus recentes
  (`SchemaTooNew`).
- Schema fidele aux entites Pony de `tribler.core.database` (v15) :
  `misc`, `torrent_state` (seeders/leechers/last_check), `tracker_state`,
  lien N-N `torrent_state_tracker`, `channel_node` (tous les champs de
  `TorrentMetadata` : `metadata_type` discriminateur, `signature` NULL
  pour les entrees FFA, contrainte `UNIQUE(public_key, id_)`), plus
  `downloads` (persistance des telechargements du daemon).
- Adaptations documentees : `datetime` → secondes Unix `INTEGER`,
  `bool` → 0/1 ; compatibilite binaire avec les bases Python non
  visee (semantique seulement).
- `channel::insert` reproduit le comportement Python : creation
  automatique du `torrent_state` associe a l'infohash.
- 5 tests en memoire (migrations, misc, sante+trackers, dedup
  channel_node, cycle downloads).

## Étape 3 — `tribler-bittorrent` : enveloppe `librqbit` (2026-09-28)

- `BtEngine` : enveloppe de `librqbit::Session` v9 (cycle de vie
  start/stop, ajout magnet/URI/bytes `.torrent`, liste, pause, reprise,
  suppression avec/sans fichiers).
- `Download` : handle léger (`Arc`) exposant id, info-hash, nom, stats.
- `DownloadStats`/`DownloadState` : types domaine découplés de
  `librqbit` (mapping `TorrentStatsState` → `Initializing/Checking/
  Downloading/Seeding/Paused/Error/Stopped`), `progress()` normalisé.
- `EngineConfig` : regroupe tous les réglages (output_dir, DHT,
  trackers, LSD, IPv4-only, port d'écoute, peer_limit, fastresume,
  proxy SOCKS5 point d'intégration `tribler-tunnel`). Constructeur
  `EngineConfig::offline` pour les tests sans réseau.
- Traduction `EngineConfig → librqbit::SessionOptions`
  (`ListenerOptions`, `ConnectionOptions::proxy_url`).
- 1 test offline (session sans réseau + ajout de `.torrent` encodé par
  `tribler-format`).

## Étape 1 — `tribler-format` : bencode, `.torrent`, magnet, `.mdblob` (2026-09-28)

- Parser bencode borné maison (`bencode/parser.rs`) : profondeur max,
  tailles de chaînes/listes/dicts limitées via `limits.rs`, offsets
  d'erreur précis, pas de dépendance réseau. Encodeur canonique
  (`bencode/encoder.rs`) avec clés de dict triées (BEP 3).
- `torrent.rs` : modèle `TorrentMeta` complet (announce, announce-list,
  fichiers multi, taille totale, private, comment, created by,
  creation date, url-list). Info-hash v1 = SHA-1 des **octets bruts**
  du dict `info` (`extract_raw_info` suit les offsets du fichier
  source, pas une re-sérialisation). Support partiel v2/hybride :
  `pieces root`, arbre `file tree`.
- `magnet.rs` : liens `magnet:?xt=urn:btih:` (hex 40c et base32) et
  `urn:btmh:` (v2), paramètres `dn`, `tr`, `ws`, `as`, `x.pe`.
- `mdblob.rs` : lecture séquentielle des `SignedPayload` pyipv8
  (`H type · H flags · 64s pubkey · champs · 64s signature`), mapping
  fidèle à `core/database/serialization.py` : `REGULAR_TORRENT`,
  `CHANNEL_TORRENT`, `COLLECTION_NODE`, `JSON_NODE`,
  `CHANNEL_DESCRIPTION`, `BINARY_NODE`, `CHANNEL_THUMBNAIL`, `DELETED` ;
  types absents du mapping Python rejetés comme
  `UnknownBlobTypeException`. Vérification de signature Ed25519 via
  `tribler-crypto`.
- 18 tests offline, `clippy -D warnings` et `fmt` propres.

## Étape 2 — `tribler-crypto` : hachage et crypto IPv8 (2026-09-28)

- `hash.rs` : SHA-1/SHA-256 + hex, validé contre vecteurs officiels.
- `ipv8/keys.rs` : clés `LibNaCLPK`/`LibNaCLSK` au format binaire pyipv8
  (préfixes `LibNaCLPK:`/`LibNaCLSK:` + paire X25519/Ed25519), MID
  = SHA-1 de la clé publique, signature/vérif Ed25519.
- `ipv8/dh.rs` : Diffie-Hellman X25519 fidèle à `ipv8-rust-tunnels`
  (`crypto_box_beforenm` + HSalsa20).
- `ipv8/session.rs` : dérivation de clés de session HKDF-SHA256
  (rôle client/serveur), compteur borné.
- Chiffrement authentifié **ChaCha20-Poly1305** (et non AES-GCM :
  correction de fidélité vs le plan initial, cf. roadmap notes).
- 16 tests (vecteurs, symétrie DH, rejet de tag invalide).

## Étape 0 — Mise en place du dépôt (2026-09-27)

- Dépôt git local initialisé (`D:\Projet\Tribler-Rust-Torrent`).
- Licence GPL-3.0-or-later (texte officiel FSF) ajoutée en `LICENSE`.
- `AGENTS.md` rédigé (règles critiques, conventions de code, workflow
  par étape, anti-duplication), inspiré de la structure du projet
  eMule-Rust de l'utilisateur.
- Analyse de l'architecture officielle Tribler
  (`D:\Projet\Tribler_sources\tribler`) : cartographie des modules
  Python (`core/libtorrent`, `core/database`, `core/restapi`,
  `core/tunnel`, `core/content_discovery`, `core/torrent_checker`,
  `core/socks5`, `core/rss`, `core/watch_folder`, `pyipv8/`).
- Recherche d'écosystème Rust : découverte de `librqbit` (moteur
  BitTorrent Rust pur mûr, Apache-2.0) et confirmation qu'aucune
  implémentation Rust complète du protocole IPv8 n'existe
  (`ipv8-rust-tunnels` ne couvre que le plan de données des tunnels).
- Décisions d'architecture actées avec l'utilisateur et formalisées en
  ADRs : ADR-0001 (moteur BitTorrent Rust pur via `librqbit`), ADR-0002
  (IPv8 inclus dès la V1), ADR-0003 (licence GPL-3.0), ADR-0004
  (structure workspace Cargo inspirée d'eMule-Rust), ADR-0005 (langue
  française).
- Workspace Cargo créé avec 12 crates squelettes
  (`tribler-format`, `tribler-crypto`, `tribler-bittorrent`,
  `tribler-ipv8`, `tribler-tunnel`, `tribler-core`, `tribler-db`,
  `tribler-network-policy`, `tribler-api`, `tribler-cli`,
  `tribler-daemon`, `tribler-test-support`) : chaque crate compile,
  documente sa responsabilité et son étape d'implémentation prévue, et
  passe `cargo check`/`cargo clippy -D warnings`/`cargo fmt --check`/
  `cargo test`.
- `docs/plans/plan_faisabilite.md`, `docs/architecture/architecture.md`,
  `docs/plans/roadmap.md`, `docs/reference_tribler/`, `docs/INDEX.md`
  rédigés.
- `scripts/verify_all.ps1` créé (validation complète).
