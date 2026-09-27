# Roadmap d'implémentation — Tribler-Rust-Torrent

Ce document est la **source de vérité de l'avancement**. Chaque étape est
cochée quand : implémentée + testée + validée manuellement + documentée +
commitée (cf. `AGENTS.md`, section "Workflow par étape"). Ne jamais
démarrer l'UI Flutter (étape 20+) avant que toutes les étapes backend
soient cochées.

Légende : `[ ]` à faire · `[~]` en cours · `[i]` implémentée et testée en
loopback, mais **interopérabilité avec un noeud pyipv8 réel non encore
validée** (la règle de cochage exige la validation manuelle — ces étapes
ne sont donc pas « terminées » au sens strict) · `[x]` terminée.

## Phase 0 — Fondations du dépôt

- [x] **Étape 0. Mise en place du dépôt.**
  Git local initialisé, licence GPL-3.0, `AGENTS.md`, workspace Cargo
  avec 12 crates squelettes (compilent, `cargo test`/`clippy -D warnings`/
  `fmt --check` passent), documentation d'architecture et de faisabilité
  initiale, script `scripts/verify_all.ps1`.

## Phase 1 — Moteur BitTorrent (via `librqbit`)

- [x] **Étape 1. Fondations formats.** `tribler-format` : parser
  bencode borné maison (zéro dépendance réseau, profondeur et tailles
  limitées), parsing `.torrent` (info-hash v1 calculé sur le dict `info`
  brut, support partiel v2/hybride : `pieces root`, `file tree`),
  liens magnet (btih hex/base32, btmh v2, trackers, webseeds, `dn`),
  blobs `.mdblob` (payloads signés Ed25519 des types du mapping Python
  `METADATA_TYPE_TO_PAYLOAD_CLASS`). 18 tests offline.
- [x] **Étape 2. Crypto de base.** `tribler-crypto` : SHA-1/SHA-256
  (vecteurs officiels), clés IPv8 `LibNaCLPK`/`LibNaCLSK` (format
  binaire pyipv8 `LibNaCLPK:`/`LibNaCLSK:` + X25519 + Ed25519, MID =
  SHA-1), DH X25519 (`crypto_box_beforenm` + HSalsa20, fidèle à
  `ipv8-rust-tunnels`), dérivation de clés de session HKDF-SHA256,
  AEAD ChaCha20-Poly1305 (pas AES-GCM : cf. note 2026-09-27), signatures
  Ed25519. 16 tests.
- [x] **Étape 3. Intégration `librqbit` et sessions de téléchargement.**
  `tribler-bittorrent` : `BtEngine` (enveloppe de `librqbit::Session`
  v9), `Download`/`DownloadStats`/`DownloadState` (types domaine
  decouples), ajout par magnet/URI/bytes `.torrent`, pause/reprise/
  suppression, `EngineConfig` (DHT, trackers, listen, proxy SOCKS5 pour
  les futurs tunnels). Test offline : session sans DHT/trackers/écoute +
  ajout de `.torrent` construit par `tribler-format`. Note : le test
  "téléchargement réel de bout en bout" reste à faire (nécessite du
  réseau ; hors scope des tests offline) — voir étape 16.
- [x] **Étape 4. Schéma SQLite et migrations.** `tribler-db` :
  `misc`, `torrent_state` (santé essaims), `tracker_state`, lien N-N
  essaim↔trackers, `channel_node` (table discriminée fidèle au mapping
  Pony v15 : metadata_type, signature nullable unique pour FFA,
  `UNIQUE(public_key,id_)` de déduplication), `downloads` (torrents
  connus du daemon). Migrations versionnées via `PRAGMA user_version`
  (SCHEMA_VERSION=1), WAL + foreign_keys. 5 tests en mémoire.

## Phase 2 — Daemon minimal et API de contrôle

- [x] **Étape 5. Session et Notifier.** `tribler-core` : `CoreSession`
  (démarrage/arrêt ordonné, restauration des téléchargements persistés,
  boucle de progression périodique bornée), `Notifier` (broadcast
  tokio borné, non-bloquant, `Lagged` pour les abonnés lents),
  `CoreConfig` centralisée. Persistance automatique des ajouts dans
  `downloads`. 2 tests offline (session en mémoire + notifier).
- [x] **Étape 6. API REST + flux d'événements minimale.** `tribler-api`
  (axum) : `GET/PUT/DELETE/PATCH /api/downloads` (list/add/remove/
  pause/resume), `GET /api/events` en **SSE** (`event: <topic>\ndata:
  <json>\n\n` — le Python utilise SSE et non WebSocket, correction de
  fidélité), erreurs au format `{"error": {handled, message}}`, DTO
  `downloads[]` miroir du dict `info` Python (codes `DownloadStatus`
  0..11 conservés). Routeur destiné au bind `127.0.0.1` (fait dans
  `tribler-daemon`). 5 tests d'intégration HTTP loopback + smoke test.
  Mapping complet : `docs/reference_tribler/api_rest_mapping.md`.
- [x] **Étape 7. CLI de pilotage minimal.** `tribler-cli` (clap +
  reqwest) : `status`, `list` (tableau infohash/statut/progression/
  débits/nom), `add` (magnet/URI → `uri`, chemin → `torrent`),
  `remove` (`--remove-data`), `pause`, `resume`. Parle uniquement à
  `tribler-api` via `--api` (défaut `http://127.0.0.1:8085`, cf.
  `DEFAULT_API`). Erreurs `{error:{handled,message}}` affichées sur
  stderr. 1 test e2e : binaire réel contre serveur API loopback
  (`status`/`list`/`add`/`pause`/`resume`/`remove` + cas injoignable).
- [x] **Étape 8. Premier daemon exécutable de bout en bout.**
  `tribler-daemon` (clap) : `--listen` (défaut `127.0.0.1:8085`,
  **refuse toute adresse non-loopback**), `--state-dir`, `--offline`
  (tests : aucun trafic sortant) ; logging `tracing`/`EnvFilter`
  (`RUST_LOG`, info par défaut) ; démarrage `CoreSession` + serveur
  axum avec graceful shutdown sur Ctrl-C (session stoppée proprement).
  1 test e2e : binaire réel spawné en `--offline`, API joignable sur
  loopback, arrêt. Le jalon "torrent réel via CLI" reste conditionné à
  un essai manuel réseau (hors tests automatiques offline).

## Phase 3 — Réseau d'anonymisation IPv8

- [x] **Étape 9. Overlay IPv8 minimal.** `tribler-ipv8` : serialiseur
  binaire pyipv8 (formats `B/H/I/Q/?`, `varlenH`, `varlenHx20`, `ipv4`,
  `ip_address`, `bits`, `raw`, `20s/…`), paquets signés au format filaire
  exact (`0x00 + version 0x02 + community_id(20o) + msg_id + varlenH(pubkey)
  + Q(global_time) + payload + sig Ed25519 64o`), `UdpEndpoint` (dispatch
  par préfixe 22o), `Peer`/`Network` (index clé/adresse, services),
  `DiscoveryCommunity` (`7e313685…df5a`) : ping/pong, similarity-request/
  response, introduction-request/response (ancien format IPv4), marche
  aléatoire périodique. 4 tests dont échange réel ping/pong loopback entre
  deux noeuds. **Preuve d'interop (bornée)** : discovery signée validée
  sur loopback contre pyipv8 (venv `D:\Projet\Tribler_sources\
  .venv-interop`, via `scripts/interop_ipv8.ps1`) — 39/39 paquets de
  l'essai acceptés dans les deux sens par le vrai `default_eccrypto`,
  pairs mutuellement enregistrés ; fixtures issues de pyipv8 rejouées
  avec succès en CI (`tests/interop_replay.rs`, `tests/fixtures/*.hex`,
  provenance : `tests/fixtures/README.md`). La cible distincte
  « Tribler 8.4.3 installé » n'est pas encore exercée.
- [i] **Étape 10. DHT overlay IPv8.** `tribler-ipv8::dht` : `calc_node_id`
  (CRC-32 IEEE d'IP masquée + `mid[:17]` — fidèle à `binascii.crc32`),
  `distance` XOR, `RoutingTable` (trie binaire, buckets de 8, split),
  `Storage` versionné, `DhtCommunity` (fusion `DHTCommunity` +
  `DHTDiscoveryCommunity`, cid `8d0be184…`) : msgs 1-10, jetons
  `sha1(str(node)+secret)` tournants, crawl itératif (8/24/4),
  puncture-request **non signé** + puncture signé, valeurs signées
  Ed25519. Tests loopback : introduction → ping → `store_value` →
  `find_values` de bout en bout entre deux noeuds. **Scénarios interop
  attendus avant `[x]`** : aller-retour DHT Rust↔Python réel exercé —
  `store_value`/`find_values` de bout en bout avec jetons tournants
  (`sha1(str(node)+secret)`) acceptés et résultat contrôlé des deux
  côtés (la couche paquet signé est déjà validée à l'étape 9 ; il reste
  les payloads DHT propres : find/store/tokens contre un
  `DHTCommunity` Python).
- [i] **Étape 11. Framework de communities complet.** `tribler-ipv8` :
  `Network` complet (`_all_addresses`/`WalkableAddress`, `discover_address`,
  `get_walkable_addresses` filtré par service/old-style,
  `get_verified_by_address`, `get_introductions_from`, `blacklist` +
  `blacklist_mids`, cache `reverse_intro` borné FIFO à 500),
  `Peer::update_clock` (Lamport + `last_response`), horloge de Lamport
  par community (`claim_global_time`/`update_global_time`, `% 65536`
  pour les introductions comme le Python), nouveaux formats
  d'introduction 233/234 (`ip_address` générique, bits
  `intro_supports_new_style` en bit 0), puncture-request **non signé**
  (250/232) → puncture signé (249/231) vers `wan_walker` (ou
  `lan_walker` si même IP WAN), `my_estimated_wan` appris depuis
  `destination_address` (`is_lan_subnet` 10/8, 172.16/12, 192.168/16,
  127/8, 169.254/16), `get_new_introduction` (pair aléatoire →
  walkable → bootstrap, 5 % de re-bootstrap). 3 tests loopback :
  introduction nouveau style, adresses walkable via introduction,
  puncture-request → puncture. **Scénarios interop attendus avant
  `[x]`** : introductions new-style (request 234 → response 233, bits
  `intro_supports_new_style` propagé) et punctures (puncture-request
  non signé 250/232 → puncture signé 249/231) échangées avec un noeud
  Python réel, **payloads décodés et vérifiés** des deux côtés — pas
  seulement la signature de paquets génériques (l'introduction ancien
  format + similarity sont déjà validées à l'étape 9).
- [x] **Étape 12. TunnelCommunity : circuits et hidden seeding.**
  `tribler-tunnel` : construction de circuits en onion routing (1/2/3
  sauts), chiffrement ChaCha20-Poly1305 par saut (`tribler-crypto`), hidden
  seeding, proxy SOCKS5 local. Jalon : téléchargement anonyme réel via
  un circuit construit contre le réseau Tribler existant.
  **Fait** : format de cellule (`cell.rs`, fidèle à `CellPayload`),
  payloads 1-20 (`payload.rs`), `Circuit`/`Hop`/`RelayRoute`/
  `UnverifiedHop` (`routing.rs`), `TunnelCommunity` (`community.rs`) :
  `create`/`created` (DH X25519+HSalsa20, `crypto_auth` HMAC-SHA-512,
  clés de session HKDF-SHA256), `extend`/`extended` avec relais
  transformé (`relay_cell` : decrypt FORWARD / encrypt BACKWARD),
  sortie UDP (`exit_data`), `destroy` signé, `ping`/`pong` de circuit,
  compteurs `relay_early` (borne 8), flags `plaintext` vérifiés.
  `UdpEndpoint` : listeners bruts par prefixe (`add_raw_prefix_listener`)
  pour les cellules non signées. Sockets de sortie dédiées
  bidirectionnelles (`exit_sockets` + tache de reception par socket ;
  retour avec `org_address` = source reelle du paquet, comme pyipv8).
  Proxy SOCKS5 (`socks5.rs`) : greeting sans auth, `UDP ASSOCIATE`,
  decapsulage des frames SOCKS5 UDP vers cellules `data`, selection
  sticky destination -> circuit `READY` du bon `goal_hops`,
  reencapsulage des reponses vers le client ; `CONNECT` : requete
  HTTP brute -> cellules `http-request`/`http-response` (msgs 28/29,
  chunks de 1400o, sortie `PEER_FLAG_EXIT_HTTP` = 32768, borne de 5
  requetes simultanees par circuit — `http_tunnel.rs`,
  `send_tcp_request` avec `Content-Length`/`chunked`).
  Hidden services (`hidden_services.rs`) : `Swarm` (info-hash ->
  points d'introduction), `establish_intro`/`intro_established`,
  `peers_request`/`peers_response` (`IntroductionInfo`), circuits E2E
  `RP_DOWNLOADER`/`RP_SEEDER` avec `required_exit`, point de rendezvous
  (`establish_rendezvous`/`rendezvous_established`, relais marque
  `rendezvous_relay` avec crypto BACKWARD propre a la route),
  `create_e2e`/`created_e2e` en paquets tunnel non signes, liaison
  `link_e2e`/`linked_e2e` (identifier partage), couche de session
  `hs_session_keys` supplementaire sur les circuits E2E (sens miroir
  downloader/seeder). Adressage des pairs caches
  `circuit_id_to_ip(cid):CIRCUIT_ID_PORT(1024)` : le SOCKS5 decode le
  circuit_id de l'IPv4 factice (`ip_to_circuit_id`) et les donnees
  entrantes des circuits RP_* sont reecrites avec cette origine
  (`data_to_socks5` des tunnels Rust). Relais UDP transparent
  (`udp_relay.rs`) : `dial` expose un circuit e2e sous une adresse
  loopback (client appris au premier datagramme — socket uTP unique
  du moteur), `serve` achemine les donnees entrantes du circuit vers
  un service UDP local et renvoie les reponses — equivalent
  SOCKS5+`udp_associate_default_remote` sans exiger SOCKS5 cote
  moteur (rqbit). Garde-fou anti-fuite `CIRCUIT_ID_PORT` :
  `is_ready_rp_circuit` exige un circuit `READY` `RP_DOWNLOADER`/
  `RP_SEEDER` (sinon rejet sans repli vers un circuit `DATA` —
  divergence volontaire par rapport a `ipv8-rust-tunnels`). Relais
  UDP en `first-seen` (le premier expediteur est verrouille). Retry
  `create_e2e` idempotent (`Swarm::pending_e2e`/`seen_e2e`/
  `in_flight_e2e`) : identifiant+DH stables sur retransmission,
  dedup cote seeder, `link-e2e` retransmis re-repondu de facon
  idempotente par le point de rendez-vous plutot que relaye comme
  donnee — corrige un doublon `RP_SEEDER` observe en test. Cause
  racine associee corrigee : `pick_first_hop` exclut desormais le
  pair `required_exit` (sinon un circuit a 2 sauts pouvait choisir
  ce meme pair comme premier ET dernier saut). 12 tests loopback :
  circuits 1/2 sauts, sortie, echo 2 sauts, destroy, SOCKS5 UDP
  ASSOCIATE (+ rejet IPv4 factice sur circuit DATA), CONNECT HTTP
  28/29, e2e hidden services complet, retry e2e sans doublon, et
  `hidden_seed_udp_relay_roundtrip`. Telechargement anonyme REEL
  valide : `tribler-bittorrent/tests/anon_download.rs` — 200 Ko
  rqbit/uTP a travers un circuit e2e lie via `udp_relay`.
  **Preuve d'interop tunnels (bornee)** : `scripts/interop_tunnel.ps1`
  — un noeud Rust cree un circuit vers le vrai `TunnelCommunity`
  pyipv8 du venv (`scripts/interop/py_tunnel_node.py`), chiffre par
  couches ChaCha20-Poly1305 accepte par le `decrypt_str` officiel,
  datagramme "uTP" sorti puis reponse re-entree par le circuit
  (cles de session identiques des deux cotes, verifiees par dump).
  Deux divergences de format corrigees a cette occasion : le
  `circuit_id` n'apparait qu'en en-tete de cellule (`body[4:]`, cf.
  `send_cell` Python) et `generate_session_keys` est HKDF
  **EXPAND_ONLY** (`Hkdf::from_prk`), pas extract+expand.
  **Suivi des flags de sortie via la decouverte** : introductions
  signees sur le prefixe tunnel (`introduction-request`/`response`,
  ancien et nouveau style) avec `extra_bytes` = bitmask
  `ExtraIntroductionPayload.flags` (`>H`, OR des `PEER_FLAG_*`) ;
  `flag_registry` (equivalent `candidates` Python) alimente
  `get_candidates(flag)` et le marquage sortie des candidats
  `created`/`extended`. `community_id` parametrable
  (`TunnelCommunity::new_with_id`, `TRIBLER_TUNNEL_COMMUNITY_ID` =
  `a3591a6b…d6bc` pour `TriblerTunnelCommunity`).
  **Interop Tribler 8.4.3 installe** : `scripts/interop_tribler.ps1`
  + `examples/tribler_relay_interop.rs` — `Tribler.exe -s` lance avec
  un etat isole (`TSTATEDIR`, `CORE_API_PORT`/`CORE_API_KEY`,
  bootstrappeurs vides — aucun trafic externe). Valide contre le
  vrai client : `introduction-request`/`response` sur le prefixe
  TriblerTunnelCommunity avec ses flags (`{RELAY, SPEED_TEST}` = 9),
  `create`/`created` puis circuit **2 sauts Rust → Tribler (relais)
  → Rust (sortie)** avec echo uTP de bout en bout (Tribler ne sort
  pas : `exitnode_enabled` non exposable — relais seul, par
  conception). **Reste** : rien de bloquant documente — l'etape est
  validee au niveau protocole (RQBIT reel via circuits e2e Rust +
  relais via le vrai client Tribler).
- [x] **Étape 13. Politiques de sécurité réseau et kill switch.**
  `tribler-network-policy` : crate de politiques pures sans dépendance
  vers `tribler-ipv8`/`tribler-bittorrent` — `address_policy::IpPolicy`
  (anti-SSRF : loopback, privé/CGNAT, link-local, multicast,
  unspecified, réservé, IPv4-mapped IPv6, ports bornés),
  `exit_policy` (port fidèle de `DataChecker`/`is_allowed` pyipv8 :
  uTP/tracker-UDP/DHT + `PEER_FLAG_EXIT_BT`, IPv8 + `PEER_FLAG_EXIT_IPV8`
  ou préfixe de la community ; source unique des constantes
  `PEER_FLAG_*` ré-exportées par `tribler-tunnel::routing`),
  `kill_switch::KillSwitch` (atomic bool + raison, `guard()`),
  `proxy_guard::validate_local_socks5_url` (loopback numérique
  uniquement). Intégrations : `tribler-tunnel::community` applique
  `is_exit_data_allowed` dans les deux sens (`exit_data` sortant et
  `exit_recv_data` rentrant, fidèle à `TunnelExitSocket`) ;
  `tribler-bittorrent::BtEngine` valide `socks5_proxy` au démarrage
  (refus = pas de démarrage, jamais de repli direct), crée un
  `KillSwitch` sondé par watchdog TCP (engage tant que le proxy est
  injoignable → `add`/`resume` refusés) ; `tribler-core` applique
  `ip_policy` aux URI `http(s)` de `Session::add_download`
  (résolution DNS + refus fermé sur toute adresse niée).
  Tests : 9 unitaires politique + `tunnel_exit_drops_non_bt_or_unflagged`
  + `policy.rs` (bittorrent : proxy distant rejeté, kill switch
  engage/release ; core : URI loopback refusée en strict, acceptée en
  permissif) — 14 tests tunnel, workspace `verify_all` vert.

## Phase 4 — Parité fonctionnelle et services secondaires

- [x] **Étape 14. Services secondaires.** `tribler-core` : équivalents de
  `content_discovery` (découverte via canaux), `torrent_checker`
  (scrape santé des torrents), `rss` (abonnements), `watch_folder`
  (import automatique de `.torrent`). Community `ContentDiscovery`
  (id `9aca62f8…1648`, msgs 3/4 santés, 101/102 version, 201/202
  remote-select) dans `tribler-ipv8` ; checker BEP-15 UDP + scrape
  HTTP avec persistance `torrent_state` et `Notification::
  TorrentHealthUpdated` ; watchers RSS conditionnels (ETag) + fetch
  anti-SSRF ; watch folder `.torrent`/`.magnet` dédupliqué. Services
  démarrés/arrêtés par `CoreSession` via `CoreConfig` — validation
  complète verte (tests loopback uniquement).
- [x] **Étape 15. Parité complète de l'API REST/SSE.** `tribler-api`
  couvre les endpoints utiles au futur client : downloads (+ sous-
  endpoints `torrent`/`trackers`/`files`/`stream`), `settings` (GET
  + POST à chaud pour RSS/watch-folder), `shutdown`, `statistics/*`
  (tribler, ipv8, dirspace), `metadata/*` (recherche locale, santé,
  popular, tags), `search/remote` (RemoteSelect IPv8), `torrentinfo`,
  `createtorrent`, `libtorrent/settings|session` (lanes anonymes),
  `ipv8/overlays` + `ipv8/tunnel/*`, `files/browse|list|create`, `rss`,
  `versioning/*`, `logging`. `CoreSession` expose la stack IPv8
  optionnelle (`ipv8_stack.rs` : endpoint UDP, discovery, content
  discovery, tunnel, SOCKS5, moteurs rqbit par lane `anon_hops`),
  persistée dans `downloads.anon_hops` (migration v2). `tribler-format`
  gagne le sérialiseur signé `mdblob::encode_entry`. Mapping complet
  dans `docs/reference_tribler/api_rest_mapping.md`.
- [x] **Étape 16. Durcissement et tests de bout en bout.** Couverture
  e2e des scénarios critiques : téléchargement réel loopback
  (`tribler-bittorrent::loopback_download`), téléchargement anonyme via
  hidden service (`anon_download`), persistance/redémarrage du daemon
  (`tribler-core::lifecycle` : DB fichier + `restore_downloads`),
  migration de schéma v1→v2 (`tribler-db::migrations` : conservation
  des données, idempotence, refus `SchemaTooNew`), kill switch +
  anti-SSRF + proxy guard (`policy.rs` dans bittorrent et core).
  `tribler-test-support` peuplé (`test_torrent_bytes`, `free_port`,
  `wait_for`) — fixtures dupliquées dedupliquées. Revue des garde-fous
  dans `docs/security/revue_garde_fous.md`.

## Phase 5 — Packaging multiplateforme du backend

- [ ] **Étape 17. Builds desktop.** Windows x64/arm64, Linux, macOS :
  scripts de build reproductibles, vérification que `tribler-daemon`
  démarre et fonctionne sur chaque plateforme cible.
- [ ] **Étape 18. Étude dédiée mobile (Android/iOS).** Modèle d'exécution
  en arrière-plan (contraintes OS), avant toute tentative de build —
  peut nécessiter d'adapter `tribler-daemon` (service léger + réveils
  périodiques plutôt que daemon permanent).
- [ ] **Étape 19. Builds mobiles.** Android puis iOS, une fois le modèle
  d'exécution validé à l'étape 18.

## Jalon "backend terminé à 100 %"

Toutes les étapes 0 à 19 doivent être cochées et validées selon les
critères de `docs/plans/plan_faisabilite.md` §8 avant de passer à la
phase suivante.

## Phase 6 — Interface Flutter (ne démarre qu'après le jalon ci-dessus)

- [ ] **Étape 20.** Plan d'architecture Flutter dédié (nouveau document),
  réutilisant les patterns de style de `C:\Emule-Sion-UI-UX\app`, ciblant
  Windows/Linux/macOS/Android/iOS/Web, consommant exclusivement
  `tribler-api`.

---

## Notes de suivi

Ajouter ici, au fil de l'avancement, tout écart constaté par rapport au
plan initial (dépendance qui ne convient pas, étape scindée en deux,
risque IPv8 sous/sur-estimé, etc.), avec la date.

- 2026-09-27 : étape 0 terminée. Découverte de `librqbit` (ADR-0001) qui
  réduit fortement le risque des phases 1-2 par rapport à l'hypothèse
  initiale d'un moteur BitTorrent écrit entièrement à la main.
- 2026-09-27 : étape 6 terminée. Correction de fidélité : l'endpoint
  `/api/events` Python est du **SSE** (`text/event-stream`), pas un
  WebSocket — `tribler-api` reproduit ce format exact. Écarts DTO
  connus consignés dans `api_rest_mapping.md` (`eta` en chaîne
  formatée, `num_seeds`/`num_connected_seeds` à 0 tant que le scraping
  trackers n'est pas implémenté).
- 2026-09-27 : étapes 1 et 2 terminées. Deux corrections de fidélité par
  rapport au plan initial : (a) le chiffrement de tunnel IPv8 est
  **ChaCha20-Poly1305** et non AES-GCM (source : `ipv8-rust-tunnels`
  `crypto.rs`) ; (b) bencode est implémenté maison dans `tribler-format`
  plutôt que via `librqbit-bencode`, pour garder le parsing borné sous
  notre contrôle (et éviter une dépendance pour un codec simple).
- 2026-09-27 : nouvelle ressource d'interop documentée — Tribler 8.4.3
  installé (`C:\Program Files (x86)\Tribler`) : `Tribler.exe` = noeud
  Tribler réel pour les tests ping-pong des étapes `[i]` et de l'étape
  12 ; `lib/` = pyipv8 figé importable dans un venv CPython 3.12.
  **Cible distincte du venv pyipv8** : son existence ne prouve aucun
  échange réussi — elle reste à exercer, et les résultats doivent être
  rapportés séparément de l'interop venv.
- 2026-09-27 : revue documentaire — les étapes 9-11 passent au marqueur
  `[i]` : elles sont implémentées et testées en loopback Rust↔Rust, mais
  l'interopérabilité avec un noeud pyipv8 réel reste à valider par un
  échange reproductible enregistré (jalon manuel, hors CI). La règle de
  cochage exige la validation manuelle ; elles ne sont donc pas
  « terminées » au sens strict. Corrections associées : le chiffrement
  de tunnel de l'étape 12 est ChaCha20-Poly1305 (et non AES-GCM, le texte
  de l'étape était resté sur le plan initial) ; les mentions
  « WebSocket » dans `plan_faisabilite.md`/`architecture.md` sont
  remplacées par SSE — le Python utilise `text/event-stream`, la mention
  WebSocket était une erreur sur la référence elle-même ; doublon de
  titre ADR-0003 corrigé (la langue française est ADR-0005) et renvoi de
  licence dans le plan de faisabilité corrigé vers ADR-0003.
