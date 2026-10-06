# Roadmap d'implémentation — OnionBit

Ce document est la **source de vérité de l'avancement**. Chaque étape est
cochée quand : implémentée + testée + validée manuellement + documentée +
commitée (cf. `AGENTS.md`, section "Workflow par étape"). La contrainte
« backend d'abord » est levée (décision du 2026-09-28) : l'UI Flutter
(`app/`) se développe en parallèle du backend, en consommant
exclusivement `onionbit-api`.

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

- [x] **Étape 1. Fondations formats.** `onionbit-format` : parser
  bencode borné maison (zéro dépendance réseau, profondeur et tailles
  limitées), parsing `.torrent` (info-hash v1 calculé sur le dict `info`
  brut, support partiel v2/hybride : `pieces root`, `file tree`),
  liens magnet (btih hex/base32, btmh v2, trackers, webseeds, `dn`),
  blobs `.mdblob` (payloads signés Ed25519 des types du mapping Python
  `METADATA_TYPE_TO_PAYLOAD_CLASS`). 18 tests offline.
- [x] **Étape 2. Crypto de base.** `onionbit-crypto` : SHA-1/SHA-256
  (vecteurs officiels), clés IPv8 `LibNaCLPK`/`LibNaCLSK` (format
  binaire pyipv8 `LibNaCLPK:`/`LibNaCLSK:` + X25519 + Ed25519, MID =
  SHA-1), DH X25519 (`crypto_box_beforenm` + HSalsa20, fidèle à
  `ipv8-rust-tunnels`), dérivation de clés de session HKDF-SHA256,
  AEAD ChaCha20-Poly1305 (pas AES-GCM : cf. note 2026-09-27), signatures
  Ed25519. 16 tests.
- [x] **Étape 3. Intégration `librqbit` et sessions de téléchargement.**
  `onionbit-bittorrent` : `BtEngine` (enveloppe de `librqbit::Session`
  v9), `Download`/`DownloadStats`/`DownloadState` (types domaine
  decouples), ajout par magnet/URI/bytes `.torrent`, pause/reprise/
  suppression, `EngineConfig` (DHT, trackers, listen, proxy SOCKS5 pour
  les futurs tunnels). Test offline : session sans DHT/trackers/écoute +
  ajout de `.torrent` construit par `onionbit-format`. Note : le test
  "téléchargement réel de bout en bout" reste à faire (nécessite du
  réseau ; hors scope des tests offline) — voir étape 16.
- [x] **Étape 4. Schéma SQLite et migrations.** `onionbit-db` :
  `misc`, `torrent_state` (santé essaims), `tracker_state`, lien N-N
  essaim↔trackers, `channel_node` (table discriminée fidèle au mapping
  Pony v15 : metadata_type, signature nullable unique pour FFA,
  `UNIQUE(public_key,id_)` de déduplication), `downloads` (torrents
  connus du daemon). Migrations versionnées via `PRAGMA user_version`
  (SCHEMA_VERSION=1), WAL + foreign_keys. 5 tests en mémoire.

## Phase 2 — Daemon minimal et API de contrôle

- [x] **Étape 5. Session et Notifier.** `onionbit-core` : `CoreSession`
  (démarrage/arrêt ordonné, restauration des téléchargements persistés,
  boucle de progression périodique bornée), `Notifier` (broadcast
  tokio borné, non-bloquant, `Lagged` pour les abonnés lents),
  `CoreConfig` centralisée. Persistance automatique des ajouts dans
  `downloads`. 2 tests offline (session en mémoire + notifier).
- [x] **Étape 6. API REST + flux d'événements minimale.** `onionbit-api`
  (axum) : `GET/PUT/DELETE/PATCH /api/downloads` (list/add/remove/
  pause/resume), `GET /api/events` en **SSE** (`event: <topic>\ndata:
  <json>\n\n` — le Python utilise SSE et non WebSocket, correction de
  fidélité), erreurs au format `{"error": {handled, message}}`, DTO
  `downloads[]` miroir du dict `info` Python (codes `DownloadStatus`
  0..11 conservés). Routeur destiné au bind `127.0.0.1` (fait dans
  `onionbit-daemon`). 5 tests d'intégration HTTP loopback + smoke test.
  Mapping complet : `docs/reference_tribler/api_rest_mapping.md`.
- [x] **Étape 7. CLI de pilotage minimal.** `onionbit-cli` (clap +
  reqwest) : `status`, `list` (tableau infohash/statut/progression/
  débits/nom), `add` (magnet/URI → `uri`, chemin → `torrent`),
  `remove` (`--remove-data`), `pause`, `resume`. Parle uniquement à
  `onionbit-api` via `--api` (défaut `http://127.0.0.1:8085`, cf.
  `DEFAULT_API`). Erreurs `{error:{handled,message}}` affichées sur
  stderr. 1 test e2e : binaire réel contre serveur API loopback
  (`status`/`list`/`add`/`pause`/`resume`/`remove` + cas injoignable).
- [x] **Étape 8. Premier daemon exécutable de bout en bout.**
  `onionbit-daemon` (clap) : `--listen` (défaut `127.0.0.1:8085`,
  **refuse toute adresse non-loopback**), `--state-dir`, `--offline`
  (tests : aucun trafic sortant) ; logging `tracing`/`EnvFilter`
  (`RUST_LOG`, info par défaut) ; démarrage `CoreSession` + serveur
  axum avec graceful shutdown sur Ctrl-C (session stoppée proprement).
  1 test e2e : binaire réel spawné en `--offline`, API joignable sur
  loopback, arrêt. Le jalon "torrent réel via CLI" reste conditionné à
  un essai manuel réseau (hors tests automatiques offline).

## Phase 3 — Réseau d'anonymisation IPv8

- [x] **Étape 9. Overlay IPv8 minimal.** `onionbit-ipv8` : serialiseur
  binaire pyipv8 (formats `B/H/I/Q/?`, `varlenH`, `varlenHx20`, `ipv4`,
  `ip_address`, `bits`, `raw`, `20s/…`), paquets signés au format filaire
  exact (`0x00 + version 0x02 + community_id(20o) + msg_id + varlenH(pubkey)
  + Q(global_time) + payload + sig Ed25519 64o`), `UdpEndpoint` (dispatch
  par préfixe 22o), `Peer`/`Network` (index clé/adresse, services),
  `DiscoveryCommunity` (`7e313685…df5a`) : ping/pong, similarity-request/
  response, introduction-request/response (ancien format IPv4), marche
  aléatoire périodique. 4 tests dont échange réel ping/pong loopback entre
  deux noeuds. **Preuve d'interop (bornée)** : discovery signée validée
  sur loopback contre pyipv8 (venv `<Tribler sources checkout> (env `TRIBLER_SRC`)\
  .venv-interop`, via `scripts/interop_ipv8.ps1`) — 39/39 paquets de
  l'essai acceptés dans les deux sens par le vrai `default_eccrypto`,
  pairs mutuellement enregistrés ; fixtures issues de pyipv8 rejouées
  avec succès en CI (`tests/interop_replay.rs`, `tests/fixtures/*.hex`,
  provenance : `tests/fixtures/README.md`). La cible distincte
  « Tribler 8.4.3 installé » n'est pas encore exercée.
- [x] **Étape 10. DHT overlay IPv8.** `onionbit-ipv8::dht` : `calc_node_id`
  (CRC-32 IEEE d'IP masquée + `mid[:17]` — fidèle à `binascii.crc32`),
  `distance` XOR, `RoutingTable` (trie binaire, buckets de 8, split),
  `Storage` versionné, `DhtCommunity` (fusion `DHTCommunity` +
  `DHTDiscoveryCommunity`, cid `8d0be184…`) : msgs 1-10, jetons
  `sha1(str(node)+secret)` tournants, crawl itératif (8/24/4),
  puncture-request **non signé** + puncture signé, valeurs signées
  Ed25519. Tests loopback : introduction → ping → `store_value` →
  `find_values` de bout en bout entre deux noeuds. **Interop réelle
  prouvée** (`scripts/interop_dht.ps1` → `INTEROP DHT OK`) contre un
  `DHTCommunity` pyipv8 : `find_values`/`store_value` **signés** dans
  les deux sens (signature vérifiée par le decodeur adverse),
  token accepté/rejeté, **rotation des secrets Python** (token
  evince → rejet, token frais → accepté). Correctif filaire associe :
  `DIST_MSG_IDS` — `GlobalTimeDistributionPayload` n'est present que
  pour les intros/punctures (246/245/234/233/249/231) ; les messages
  DHT `ez_send` sont `[auth, payload]` sans `dist`
  (`Packet::sign_no_dist`/`sign_auto`).
- [x] **Étape 11. Framework de communities complet.** `onionbit-ipv8` :
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
  puncture-request → puncture. 3 tests loopback :
  introduction nouveau style, adresses walkable via introduction,
  puncture-request → puncture.
  **Interop prouvée** (`scripts/interop_discovery.ps1` →
  `INTEROP DISCOVERY OK`) : contre un vrai `DiscoveryCommunity`
  pyipv8 — Rust→Python : 234 décodé+répondu 233 (flag
  `new_style_intro` propagé), 232→231 et 250→249 ; Python→Rust :
  234→233 (réponse décodée + signature vérifiée), 232→231 et
  250→249 — chaque handler pyipv8 passe par `lazy_wrapper` (decode +
  signature). Corrections de fidélité faites à cette occasion :
  `add_verified` inscrit l'adresse dans `_all_addresses`
  (`WalkableAddress(b"", None, False)`), ne vérifie pas un pair à
  adresse blacklistée inconnue, et fusionne `new_style_intro` à la
  mise à jour d'un pair connu (objet partagé Python). Observables
  ajoutés : `intro_request_count`/`intro_response_count`/
  `puncture_count` + `send_puncture_request` publique.
- [x] **Étape 12. TunnelCommunity : circuits et hidden seeding.**
  `onionbit-tunnel` : construction de circuits en onion routing (1/2/3
  sauts), chiffrement ChaCha20-Poly1305 par saut (`onionbit-crypto`), hidden
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
  valide : `onionbit-bittorrent/tests/anon_download.rs` — 200 Ko
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
  conception). **Critere clos — banc retenu** : le seul element
  bloquant etait l'absence d'une sortie Tribler reelle
  (`exitnode_enabled` non exposable par configuration du client
  installe) ; le banc `scripts/interop_exit_download.ps1` +
  `examples/exit_download_interop.rs` (onionbit-bittorrent) valide le
  scenario retenu — telechargement rqbit **reel** (200 Ko, uTP) a
  travers un circuit a 2 sauts dont le dernier saut est le vrai
  `TunnelCommunity` pyipv8 en sortie (`PEER_FLAG_EXIT_BT`,
  `scripts/interop/py_tunnel_node.py`, `--echo-port` devenu
  optionnel) : downloader → relais Rust → sortie pyipv8 → socket
  uTP du seeder, contenu verifie octet a octet. `udp_relay::dial_to`
  ajoute la variante a destination filaire explicite (adresse reelle
  du seeder, au lieu de l'IPv4 factice des circuits e2e).
  `INTEROP EXIT DOWNLOAD OK`.
- [x] **Étape 13. Politiques de sécurité réseau et kill switch.**
  `onionbit-network-policy` : crate de politiques pures sans dépendance
  vers `onionbit-ipv8`/`onionbit-bittorrent` — `address_policy::IpPolicy`
  (anti-SSRF : loopback, privé/CGNAT, link-local, multicast,
  unspecified, réservé, IPv4-mapped IPv6, ports bornés),
  `exit_policy` (port fidèle de `DataChecker`/`is_allowed` pyipv8 :
  uTP/tracker-UDP/DHT + `PEER_FLAG_EXIT_BT`, IPv8 + `PEER_FLAG_EXIT_IPV8`
  ou préfixe de la community ; source unique des constantes
  `PEER_FLAG_*` ré-exportées par `onionbit-tunnel::routing`),
  `kill_switch::KillSwitch` (engagements **scopés** par portée
  `proxy`/`circuits`/`manuel` — un proxy redevenu joignable ne
  désarme pas une panne de circuits — `guard()`),
  `proxy_guard::validate_local_socks5_url` (loopback numérique
  uniquement). Intégrations : `onionbit-tunnel::community` applique
  `is_exit_data_allowed` dans les deux sens (`exit_data` sortant et
  `exit_recv_data` rentrant, fidèle à `TunnelExitSocket`) ;
  `onionbit-bittorrent::BtEngine` valide `socks5_proxy` au démarrage
  (refus = pas de démarrage, jamais de repli direct), crée un
  `KillSwitch` sondé par watchdog TCP (portée `proxy` : engage tant
  que le proxy est injoignable → `add`/`resume` refusés) ;
  `onionbit-core` ajoute un watchdog **circuits** par lane anonyme
  (portée `circuits`, événementiel via
  `TunnelCommunity::watch_circuits` : proxy joignable ≠ circuit
  disponible) ; `onionbit-core` applique
  `ip_policy` aux URI `http(s)` de `Session::add_download`
  (résolution DNS + refus fermé sur toute adresse niée).
  Tests : 9 unitaires politique + `tunnel_exit_drops_non_bt_or_unflagged`
  + `policy.rs` (bittorrent : proxy distant rejeté, kill switch
  engage/release ; core : URI loopback refusée en strict, acceptée en
  permissif) — 14 tests tunnel, workspace `verify_all` vert.

## Phase 4 — Parité fonctionnelle et services secondaires

- [x] **Étape 14. Services secondaires.** `onionbit-core` : équivalents de
  `content_discovery` (découverte via canaux), `torrent_checker`
  (scrape santé des torrents), `rss` (abonnements), `watch_folder`
  (import automatique de `.torrent`). Community `ContentDiscovery`
  (id `9aca62f8…1648`, msgs 3/4 santés, 101/102 version, 201/202
  remote-select) dans `onionbit-ipv8` ; checker BEP-15 UDP + scrape
  HTTP avec persistance `torrent_state` et `Notification::
  TorrentHealthUpdated` ; watchers RSS conditionnels (ETag) + fetch
  anti-SSRF ; watch folder `.torrent`/`.magnet` dédupliqué. Services
  démarrés/arrêtés par `CoreSession` via `CoreConfig` — validation
  complète verte (tests loopback uniquement).
- [x] **Étape 15. Parité complète de l'API REST/SSE.** `onionbit-api`
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
  persistée dans `downloads.anon_hops` (migration v2). `onionbit-format`
  gagne le sérialiseur signé `mdblob::encode_entry`. Mapping complet
  dans `docs/reference_tribler/api_rest_mapping.md`.
- [x] **Étape 16. Durcissement et tests de bout en bout.** Couverture
  e2e des scénarios critiques : téléchargement réel loopback
  (`onionbit-bittorrent::loopback_download`), téléchargement anonyme via
  hidden service (`anon_download`), persistance/redémarrage du daemon
  (`onionbit-core::lifecycle` : DB fichier + `restore_downloads`),
  migration de schéma v1→v2 (`onionbit-db::migrations` : conservation
  des données, idempotence, refus `SchemaTooNew`), kill switch +
  anti-SSRF + proxy guard (`policy.rs` dans bittorrent et core),
  fuite en plein transfert sous deux scénarios distincts : proxy mort
  (`onionbit-bittorrent::kill_switch_midtransfer`) et **circuit détruit
  proxy vivant** (`onionbit-core::circuit_death` — portée `circuits`
  du kill switch, zéro datagramme vers la destination pendant la
  fenêtre morte, reprise sur nouveau circuit).
  `onionbit-test-support` peuplé (`test_torrent_bytes`, `free_port`,
  `wait_for`) — fixtures dupliquées dedupliquées. Revue des garde-fous
  dans `docs/security/revue_garde_fous.md`.

## Phase 5 — Packaging multiplateforme du backend

- [x] **Étape 17. Builds desktop.** `scripts/build_release.ps1`
  (release reproductible + `dist/<target>/` + `build-manifest.json`
  version/commit/rustc). **Windows x64 vérifié** : `cargo build
  --release` + smoke du daemon (API loopback + shutdown). Cibles
  ARM64-Windows/Linux/macOS supportées par le script mais **non
  vérifiables sur cette machine** : toolchain MSVC ARM64 absente
  (pas de `Hostx64/arm64/cl.exe`), pas de cross-gcc Linux, SDK Apple
  requis. → CI matricielle requise pour valider les autres cibles.
- [x] **Étape 18. Étude dédiée mobile (Android/iOS).** Modèle
  d'exécution documenté (`docs/plans/mobile_execution_model.md`) :
  pas de daemon permanent possible — façade FFI `tribler-mobile` +
  service de premier plan ; adaptations requises listées
  (`pause_all`/`resume_all` — **implémentés** —, callbacks FFI du
  `Notifier`, anonymat off par défaut, pas de seeding continu ni
  d'exit node). Surface FFI figée dans
  `docs/plans/mobile_ffi_surface.md` (fonctions plates JSON,
  callback notifications, règles suspension/arrêt/reprise).
- [ ] **Étape 19 — remplacée (2026-09-28).** Pas de daemon mobile :
  Android/iOS seront une **interface de pilotage à distance**
  (consommant `onionbit-api` REST+SSE sur un daemon desktop),
  développée après l'interface desktop. La façade FFI
  `tribler-mobile` et ses builds ne sont plus nécessaires — le
  document `mobile_ffi_surface.md` reste comme référence si le
  besoin d'un moteur embarqué réapparaît ; `pause_all`/`resume_all`
  restent utiles au daemon desktop (arrêt rapide, suspension).

## Phase 5b — Parité complète de l'API de contrôle (daemon)

Câblage dans le daemon de tous les endpoints restants de
`docs/reference_tribler/api_endpoints_complet.md` (daemon uniquement,
pas d'UI). Décisions du 2026-09-28 : clé API `X-Api-Key`/`?key=`/cookie
activée en parité Python (loopback compris — `onionbit-cli` la lit dans
`configuration.json`) ; `asyncio/*` adapté au runtime tokio ;
`identity/*` Tribler (SSI/attestations) exclu — nos routes
`/api/identity/*` sont une extension propre (ADR-0016, étape 48) ;
`GET /api/rss` (items) implémenté.

- [x] **Étape 21. Configuration persistée et clé API.**
  `DaemonConfig` serde = arbre `TriblerConfig` (défauts du doc :
  `api`, `ipv8`, `libtorrent`+`download_defaults`, `tunnel_community`,
  `rss`, `watch_folder`, `torrent_checker`, `dht_discovery`,
  `versioning`, `statistics`, `state_dir`) lu/écrit dans
  `state_dir/configuration.json` ; flags CLI = overrides ;
  `api/key` générée hex au premier run, `api/http_port_running`
  réécrit après bind réel. Middleware auth axum (header `X-Api-Key`,
  `?key=`, cookie `api_key` → 401 `{error:{handled:true}}`) ; pas de
  chemins exemptés (`/docs`/`/ui` absents). `onionbit-cli --api-key` +
  lecture auto du fichier. `GET /api/settings` = arbre complet ;
  `POST /api/settings` = merge récursif + persistance disque +
  application à chaud. `DefaultBodyLimit` aligné à 16 Mio
  (`MAX_REQUEST_SIZE` Python).
- [x] **Étape 22. Réglages par download persistés + PATCH complet.**
  Migration `onionbit-db` v3 (`downloads` : `safe_seeding`,
  `upload_limit`, `download_limit`, `seeding_ratio`, `auto_managed`,
  `queue_position`, `completed_dir`, `selected_files`, `trackers`) →
  DTO complété. `PATCH` : `selected_files` (`Session::update_only_files`
  rqbit), `state=recheck`/`move_storage` (stop + déplacement + ré-add
  re-hashé), `upload_limit`/`download_limit` (`ratelimits` rqbit par
  torrent + session), `seeding_ratio(+_default)`/`seeding_mode`
  (politique d'arrêt de seed dans `onionbit-core`).
  `queue_position`/`auto_managed`/`file_priority` : sémantique
  simplifiée documentée (pas d'équivalent rqbit — ADR si substantiel).
- [x] **Étape 23. Trackers et flags d'enrichissement du listing.**
  `PUT …/default_trackers` (`download_defaults/trackers_file`),
  `DELETE …/trackers`, `PUT …/tracker_force_announce` (via
  `tracker_comms` ou re-application à la liste persistée — divergence
  documentée si rqbit ne l'expose pas à chaud). Les flags
  `get_peers`/`get_pieces`/`get_availability` de `GET /api/downloads`
  ont été livrés avec l'étape 22.
- [x] **Étape 24. Topics SSE complets.** Nouvelles variantes
  `Notification` + émetteurs : `remote_query_results`
  (`uuid,query,results,peer`), `local_query_results`, `tunnel_removed`,
  `tribler_shutdown_state` (progression de `stop()`), `low_space`
  (sonde disque périodique du `saveas`), `tribler_new_version`,
  `tribler_exception`, `ask_add_download` (`ask_download_settings` +
  `cli`), `report_config_error`. `events_start.public_key` =
  `Ipv8Stack::public_key_hex()`.
- [x] **Étape 25. `DhtCommunity` dans la stack + `/api/ipv8/dht/*`.**
  `Ipv8Stack.dht` (`dht_discovery/enabled` → `DHTDiscoveryCommunity`),
  `walk_to` au bootstrap, maintenance `step`/`node_maintenance`/
  `value_maintenance`/`token_maintenance` aux cadences Python
  (0,5 s/60 s/3600 s/300 s) + propagation `my_estimated_wan/lan`.
  7 routes au comportement pyipv8 (404 community absente, 200
  `{"buckets":[]}`, 400 `no such bucket`, PUT `{"value"}` 400/500,
  `distance` en décimale 160 bits).
- [x] **Étape 26. IPv8 réseau et diagnostics.** `GET /api/ipv8/network`
  (pairs vérifiés, `{b64(mid): {ip, port, public_key, services}}`),
  `POST /api/ipv8/isolation` (`bootstrapnode` → blacklist globale +
  par overlay + `walk_to` + `DispersyBootstrapper.ip_addresses` ;
  `exitnode` → `walk_to` tunnel ; `exitnode` prioritaire si les deux),
  `GET /api/ipv8/noblockdht/{mid}` (`connect_peer` fire-and-forget,
  404 sans community, 500 sur hex invalide), `GET`/`POST
  `/api/ipv8/overlays/statistics` (`StatisticsEndpoint` :
  `NetworkStat` par prefixe/`msg_id` rx+tx avec timestamps,
  agregat `diff_time`, decode_map par community + `:unknown`,
  stats auto-activees au demarrage comme `session.py`, POST 400/412),
  `GET /api/ipv8/overlays` reecrit au `OverlaySchema` complet
  (`my_peer`, `global_time`, `max_peers`, `is_isolated`,
  `my_estimated_*`, `strategies` RandomWalk/RandomChurn/
  PeriodicSimilarity) ; handlers tunnel alignes Python (collections
  vides en 200 quand `tunnels is None`).
- [x] **Étape 27. Tunnel avancé.** `GET …/swarms/{ih}/size`
  (`estimate_swarm_size` : crawl `peers-request` itératif, DHT puis
  IPs PEX, comptage des `seeder_pk` uniques de source PEX — quirk
  conservé : `?hops=` arrive en chaîne → `swarm_size` 0),
  `peers/dht` (`DHTIntroPointPayload` `["ip_address","I","varlenH",
  "varlenH"]` décodé depuis le `Storage` DHT local, `PackError`
  ignorée), `peers/pex` (store `PexStore` par info_hash alimenté par
  `on_establish_intro`/`stop_announce`, TTL 300 s, borne 20) ;
  `GET …/circuits/test` + `/{cid}/test` : `run_speedtest` dans
  `TunnelCommunity` (cellules `test-request`/`test-response`
  **21/22 u32** d'`ipv8-rust-tunnels` — le filaire réel de Tribler
  8.x — plus 19/20 u16 du backend Python pur ; `send_cell` renvoie
  les octets émis ; flux `text/event-stream` de lignes
  `speed: {"up","down"}` MiB/s, validation `goal_hops`/`test_time_ms`,
  circuit `READY` + `PEER_FLAG_SPEED_TEST` sur `DATA`, suppression
  du circuit de test après 5 s de `remove_tunnel_delay`).
- [x] **Étape 28. `asyncio/*` adapté à tokio + RSS items + clôture.**
  `/api/ipv8/asyncio/drift` (dérive des intervalles périodiques,
  historique 100), `/tasks` (registre des tâches nommées du daemon),
  `/debug` GET/PUT (`EnvFilter` rechargé à chaud). `GET /api/rss` :
  table `rss_items` + listing alimenté par le `RssService`. Banc de
  parité `scripts/api_parity.ps1` (même batterie de requêtes contre
  `Tribler.exe -s` et le daemon Rust, diff des réponses) ; mise à jour
  `api_endpoints_complet.md`/`api_rest_mapping.md` ; ADR pour les
  écarts résiduels (exclusion `identity/*` comprise).

## Phase 5c — Cycle de vie desktop du daemon

- [x] **Étape 29. Systray Windows et arrêt unifié du daemon.**
  `onionbit-daemon` passe en sous-système GUI
  (`#![windows_subsystem = "windows"]` — plus aucune fenêtre console
  au lancement) : icône de zone de notification (`tray-icon` sur un
  thread dédié à pompe de messages Win32) avec menu « Ouvrir
  Tribler » (lance `onionbit_ui.exe` à côté de l'exe), « Démarrer
  avec Windows » (valeur `HKCU\...\Run` via `winreg`, état coché
  reflété en direct), « Ouvrir le dossier des logs », « Quitter ».
  Icône `onionbit.ico` embarquée via `embed-resource` + `resources.rc`
  (ressource 101, sert aussi d'icône de l'exe). Arrêt unifié :
  `ShutdownSignal` (`tokio::sync::Notify`) déclenché par Ctrl-C, «
  Quitter » ou `PUT /api/shutdown` — le endpoint termine désormais
  réellement le processus (écart corrigé : avant il ne stoppait que
  la session, `arreter.cmd` finissait au `taskkill`) ;
  `CoreSession::stop()` rendu idempotent (`stopped: AtomicBool`).
  Mutex d'instance unique par `state_dir`
  (`Local\TriblerRustDaemon-{hash}` — un second lancement sort
  silencieusement, pas de double icône). Flags `--console`
  (AttachConsole/AllocConsole + handles `CONOUT$`, pour le debug) et
  `--no-tray` ; réglage persisté `tray/enabled` dans
  `configuration.json`. `demarrer.ps1` (template `build_dist.ps1`)
  sans `-WindowStyle`. **Validation visuelle faite** (2026-09-30) :
  menu systray, bascule autostart et « Quitter » vérifiés à la souris.

## Jalon "backend terminé à 100 %"

Toutes les étapes 0 à 18 cochées, étape 12 comprise : le critère
« téléchargement via le réseau Tribler existant » est **validé** —
le banc `scripts/interop_exit_download.ps1` a été rejoué le
2026-09-29 contre le `TunnelCommunity` pyipv8 réel en sortie
(`PEER_FLAG_EXIT_BT`) : 200 Ko rqbit/uTP à travers un circuit
2 sauts, contenu vérifié octet à octet (`INTEROP EXIT DOWNLOAD OK`),
puis étendu le 2026-09-30 à un premier saut assuré par **Tribler.exe
8.4.3 réel** (route épinglée vérifiée, 4 Mio à 3 sauts). Étapes 20
et 29 également clôturées : validation visuelle faite — **toute la
roadmap est terminée**.

## Phase 6 — Interface Flutter desktop

- [x] **Étape 20. Plan d'architecture Flutter + coquille
  implémentée** — `docs/plans/flutter_architecture.md`, patterns de
  `the reference Flutter UI project`, consommant exclusivement `onionbit-api`
  (REST + SSE). Cible immédiate : **Windows desktop** ; Linux/macOS
  ensuite ; le mobile distant reprendra la même base (étape 19
  remplacée). Première passe livrée dans `app/` : thème eMule (seed
  `0xFF2F6FED`, accents, jour/nuit/auto persistés), shell responsive
  (sidebar Tribler ≥ 600 dp / `NavigationBar` en dessous), features
  downloads + search + settings + diagnostic. `flutter analyze`
  propre, 8 tests verts, `flutter build web` et `flutter build
  windows` OK. **Intégration API validée en live** (2026-09-28) :
  les 11 endpoints REST consommés par l'UI (`downloads`, `settings`,
  `ipv8/overlays`, `tunnel/{circuits,relays,exits,swarms,peers}`,
  `metadata/{popular,search/local}`, `logging`, `events`) répondent
  200 contre le daemon réel, SSE `events_start` reçu, et la
  résolution clé/port depuis `configuration.json` est conforme à ce
  que le resolver (`daemon_api_resolver_native.dart`) et
  `dist/demarrer.ps1` attendent. **Seconde passe (2026-09-28)** —
  toutes les fonctions du daemon exposées dans l'UI : file
  d'attente par téléchargement (`queue_position` monter/descendre/
  haut/bas, `auto_managed`), limites de débit individuelles, ratio
  de seed individuel/défaut, `recheck`, `move_storage`, inclusion
  et priorité par fichier, trackers (retrait, annonce forcée,
  trackers par défaut) ; onglets Diagnostic « Statistiques »,
  « Pairs DHT », « Pairs PEX » et test de vitesse de circuit
  (flux `speed:` pyipv8) ; sections Réglages « Bande passante »
  (limites globales), « File d'attente » (`active_*`), « Seed &
  anonymat par défaut » (mode/ratio/durée/hops/safe seeding),
  « Tunnels anonymes » (`min/max_circuits`, noeud de sortie),
  « Réseau » (DHT/UPnP/NAT-PMP/LSD/uTP + proxy), « Automatisation »
  (watch folder + flux RSS avec items découverts), « Mises à jour »
  (version + sonde) et indicateur d'espace disque. **Validation
  visuelle faite** (2026-09-30) : rendu réel contre le daemon
  vérifié à la souris.
- [x] **Étape 30. Internationalisation complète de l'app** —
  `flutter_localizations` + `intl` + `gen_l10n`, gabarit
  `app_en.arb` + `app_fr.arb` (~300 clés), locale persistée
  (`ui.locale`) avec **anglais par défaut** et bascule EN/FR à
  chaud dans Réglages → Apparence. Extraction intégrale des ~300
  littéraux (core, search, diagnostic, settings, downloads),
  formatteurs sensibles à la locale, pluriels ICU. Garde-fou
  `scripts/check_i18n.ps1`. Détails : ADR-0009, CHANGELOG.

## Phase 7 — Interface web (Flutter web)

Portage de `app/` vers la cible web, **servi par `onionbit-daemon` en
same-origin** (modèle des exemptions `/ui`/`/static` de
l'`ApiKeyMiddleware` Python — pas de CORS, aucun affaiblissement de
l'auth ni du bind loopback). Plan détaillé :
`docs/plans/web_ui_plan.md`.

- [x] **Étape 31. Compatibilité de compilation et transports web.**
  `fetch_client` derrière un import conditionnel (SSE `/api/events` et
  speed test streamés — `BrowserClient` XHR bufferise tout) ;
  abstraction `pick_file` (web = `<input type=file>` → octets pour
  `putTorrent` ; desktop = `file_selector` inchangé) ; chemins de
  dossier en saisie texte sur web (chemins du daemon, pas du
  navigateur) ; `desktop_drop` no-op web ; guards `kIsWeb` résiduels ;
  `flutter build web` (+ analyze/test) ajoutés à `verify_all.ps1`.
- [x] **Étape 32. Connexion et authentification web.**
  `baseUrl` = `Uri.base.origin` quand l'UI est servie par le daemon ;
  dialogue de saisie de la clé API (lue dans `configuration.json`),
  persistance `shared_preferences`, option `?key=`/cookie `api_key` ;
  `ensureDaemonRunning`/`rediscover` no-op web ; masquage des
  éléments desktop (lancement auto, bandeau daemon).
- [x] **Étape 33. Service des statiques dans le daemon.**
  `ServeDir`/`rust-embed` : `GET /` → `index.html`, assets Flutter,
  fallback SPA pour les liens profonds `go_router` ; exemption d'auth
  limitée aux statiques (parité Python `/ui`+`/static`), `/api/*`
  toujours protégé ; config `api/web_ui_enabled` + `api/web_ui_dir` ;
  en-têtes `nosniff`/`Cache-Control` ; tests (statique servi,
  fallback, 401 `/api` sans clé, anti-traversée).
- [x] **Étape 34. Adaptations UX web.** Drop HTML5 → `putTorrent` ;
  sélecteur de dossiers côté daemon via `/api/files/browse`/`list`
  (destination, watch folder, `move_storage`) ; streaming
  `/stream/{i}?key=` dans un onglet (player HTML5) ; notifications
  navigateur ; `manifest.json`/`index.html` réels (PWA) ; validation
  responsive navigateur/mobile.
- [x] **Étape 35. Packaging et parcours utilisateur.**
  `build_dist.ps1`/`build_release.ps1` : `flutter build web` →
  `dist/<target>/web/` ; systray « Ouvrir dans le navigateur » ;
  docs utilisateur (clé API, `?key=`, UI locale vs web) ; validation
  manuelle multi-navigateurs du parcours complet.

Phase 7b optionnelle (exposition LAN, `api/cors_origins` dev, pilote
mobile distant) : voir `web_ui_plan.md` — hors scope V1.

## Phase 8 — Messagerie anonyme sur circuits e2e (ADR-0011)

ADR-0011 **acceptée** (questions tranchées 2026-10-02). Séquence
imposée : codec/signatures → transport e2e → consentement/anti-abus
→ persistance → API/UI → bancs. Chaque étape livre ses tests
négatifs **avec** la fonction — aucune classe de trames sans
contrepartie hostile testée. Bancs : `docs/plans/bancs_tests.md`
§4.11 (`MS-*`).

- [x] **Étape 36. Codec et crypto applicative** — fait (2026-10-05,
  crate `onionbit-messaging`). Trame bencode
  déterministe `{v, type, id, seq, ts, body, sig}` (≤ 32 Kio sur le
  fil, `body` ≤ 30 Kio, rejet `v != 1` et champs critiques
  inconnus) ; signature Ed25519 par la clé IPv8 de l'expéditeur
  vérifiée contre la `pk` dont dérive `messaging_hash(pk)` ;
  dérivation applicative HKDF-SHA256 domaine `"onionbit messaging
  v1"` séparée de `hs_session_keys` ; compteur `seq` u64 + fenêtre
  de réception 64 + dédup `id` 128 bits. Tests : roundtrip strict,
  déterminisme de l'encodage, rejets (malformé, taille, version,
  signature invalide, seq rejoué/hors fenêtre, `id` dupliqué),
  cible fuzz `messaging_frame` + miroir stable proptest (MS-3,
  MS-4, MS-5).
- [x] **Étape 37. Transport e2e et démultiplexage** — fait
  (2026-10-05, `services/messaging.rs` dans `onionbit-core`).
  `MessagingService` : présence par `join_swarm_with_key(
  messaging_hash(pk), hops, sk_identité)` — le `seeder_pk` publié
  est la clé du destinataire et `created-e2e` l'authentifie au
  niveau transport ; moniteur de présence (IP épinglés via
  `ensure_introduction_points`, re-annonce périodique) ;
  résolution `send_peers_request` (PEX sur IP connu ou DHT) +
  `create_e2e` (`RP_DOWNLOADER`) + attente `e2e_ready` côté
  expéditeur ; démultiplexage `subscribe_circuit_data` par
  `info_hash` = `messaging_hash(pk)` sur les swarms messagerie —
  un circuit e2e = une conversation, jamais de mux uTP (la lane
  BitTorrent ignore les swarms hors `swarm_lookup`, et les trames
  ne passent pas `could_be_utp`) ; clés applicatives HKDF
  domaine-séparé posées à la liaison (`e2e_shared_secret` exposé
  par le tunnel au `created-e2e`/`linked-e2e`). Identification :
  `hello` lie un circuit au `pk` déclaré **vérifié par
  signature**, sinon la trame est éprouvée contre chaque contact
  connu. Activable par `tunnel.enable_messaging` (défaut off,
  nécessite `enable_anonymity`). Tests : cycle complet loopback
  2 nœuds avec présence+IP+liaison+trames bidirectionnelles,
  dédup à travers réouverture de circuit, filtre uTP (MS-1,
  MS-2, MS-9).
- [x] **Étape 38. Consentement et anti-abus** — fait (2026-10-05,
  `services/messaging.rs` + `preflight` dans `onionbit-messaging`).
  Machine d'état `ContactState { Active, Pending, Blocked }` :
  `hello` vérifié d'un inconnu → `pending` borné (`pending_cap=64`,
  `pending_ttl=600 s`, purge au tick et à l'admission) + événement
  `Consent` ; `accept_contact` → `Active` + trame `accept` +
  `Bound` ; `refuse_contact` → `reject` + oubli (re-`hello`
  repropose) ; `block_contact` → trames comptées `blocked`,
  circuit détruit, swarm désarmé (jamais joint), `resolve`/
  `connect`/`send` refusés ; trames d'un `pending` vérifiées mais
  jamais livrées (`pending_drop`) ; `hello` jamais livré (contrôle).
  Budgets : seau global avant le codec (`global_rate=10/s`) et seau
  par contact (`per_contact_rate=2/s`) — dédup `id` gratuite avant
  le seau, `admit` après (une trame écartée au budget reste
  livrable par réémission). Préfiltre `preflight` : taille +
  suffixe de version canonique `1:vi<ver>ee` avant tout parse
  bencode. Compteurs `MessagingStats` exposés. Tests : cycle
  consentement complet, refus→re-proposition, blocage persistant,
  `pending` plein/TTL, budgets contact+global, préfiltre (MS-6,
  MS-10).
- [x] **Étape 39. Persistance et livraison** — fait (2026-10-05,
  migration v15 + `onionbit-db::messaging` + hooks du service).
  Tables `msg_contacts` (pk, état, `send_seq`, `recv_top`,
  `retention_secs`, `secure_delete`) et `msg_messages` (id, fk
  cascade, direction, seq, ts, body, status
  `received|sent|acked|failed`) — persistance en clair v1 assumée.
  Suppression réelle `DELETE` (contact = cascade) ; rétention par
  contact purgée au tick (`secure_delete` zeroise le corps avant
  le `DELETE`). ACK applicatif (`body` = `id` acquitté) → statut
  `acked` ; exempté du seau contact (contrôle vérifié, ne doit pas
  faire perdre de `msg`). Offline → erreur + événement
  `Undeliverable` + ligne `failed` visible (pas de file ni de
  réémission). Restart : `load_state` restaure état, `send_seq` et
  `recv_top` (`RecvWindow::resume` conservateur). API service :
  `history`, `stored_contacts`, `set_retention`, `delete_message`.
  Tests : cycle DB complet, offline `failed`, restart (état +
  anti-replay repris), ACK → `acked`, rétention/suppression réelle
  (MS-7, MS-11).
- [x] **Étape 40. API REST, SSE et UI** — fait (2026-10-05).
  Extension Rust `/api/messaging/*` (pas de parité Python) :
  `GET /stats` (clé locale + hash de présence + compteurs de drops),
  `GET /contacts` + `GET /contacts/pending`,
  `POST /contacts/connect` (résolution DHT/PEX + `create-e2e`),
  `POST /contacts/{pk}/accept|refuse|block` + `DELETE .../block` +
  `DELETE /contacts/{pk}`, `GET/POST /contacts/{pk}/messages`
  (historique borné + envoi — `404` = hors ligne, enregistré
  `failed`), `DELETE /messages/{id}` (suppression réelle),
  `POST /contacts/{pk}/retention`. Flux SSE dédié
  `GET /messaging/events` (`messaging_frame|bound|pending|consent|
  undeliverable` depuis le `broadcast` du service — jamais le
  `Notifier` global, la messagerie reste opt-in). Tout endpoint
  répond `404 « messagerie desactivee »` quand le service est off
  (MS-12 — test exhaustif des 14 routes). UI Flutter `features/
  messaging` : onglet Messages (`NavId.messages`, `/messages`),
  liste contacts + section demandes pending (accept/refuse/block),
  conversation (historique inversé, statuts envoyé/livré/non livré,
  suppression au clic long), menus blocage/déblocage/rétention/
  suppression, `SseClient` généralisé par `path`, pont SSE →
  invalidation pull. `flutter analyze` propre ; clés l10n
  en/fr ajoutées.
- [x] **Étape 41. Validation sécurité de la phase** — fait
  (2026-10-05). Inventaire `MS-1..MS-12` couvert : MS-1/MS-2/MS-9
  (loopback e2e, réouverture, séparation de lane), MS-3/MS-4/MS-5
  (auth Ed25519, codec hostile + fuzz `messaging_frame`,
  anti-replay), MS-6/MS-10 (consentement borné, budgets + drops
  comptés), MS-7/MS-11 (offline `failed`, restart + DELETE
  physique), MS-12 (14 routes → 404 off, 401 sans clé). **MS-8
  mesuré** : `fingerprint_mesh.ps1 -WithMessaging` (switch ajouté)
  15 min — présence seule = +1 859 cellules tunnel (+167 %, ~2,1
  cells/s) sur 4 nœuds, aucune boucle de contrôle, consigné dans
  `fingerprinting.md`. Threat model : section messagerie
  (démontré vs non-claims — métadonnée de présence assumée, pas de
  forward secrecy, persistance en clair, identité partagée).
  **MS-13 vert** (2026-10-05) : `interop_messaging_e2e.ps1` — cycle
  applicatif réel entre deux démons (mesh ancre/relai/exit), vert en
  `messaging_hops=1` et `hops=2`. Bugs trouvés par le banc et
  corrigés : `SCHEMA_VERSION` de `onionbit-db` désormais
  `MIGRATIONS.len()` (restart refusait la DB v15), fermeture des
  canaux `subscribe_circuit_data` à la mort/destruction d'un
  circuit (la messagerie ne déliait jamais), refus d'`extend` vers
  soi-même + exclusion du RP par adresse en plus de la clef,
  cadence `ip_check_interval` (10 s) pour les points d'introduction
  (un premier essai pré-vérification n'était retenté qu'à 300 s).

---

## Phase 9 — Extensions OnionBit-only (ADR-0015)

Le protocole partagé avec Tribler 8.x (« legacy ») est figé ; les
fonctions nouvelles parlent uniquement entre nœuds OnionBit, from
scratch en Rust (TrustChain upstream est retiré depuis 2024 — aucun
format historique à reprendre). Séquence : comptabilité locale →
communauté d'extension → ledger signé → curation → anti-DPI. Même
discipline de tests que la Phase 8 : aucune fonction sans sa
contrepartie hostile.

- [x] **Étape 42. Comptabilité locale par pair (`peer_stats`)** —
  `onionbit-tunnel::peer_stats` : compteurs `bytes_served` (circuits
  joints par `create` direct — l'initiateur est le `requester`, seul
  le premier saut le connaît) et `bytes_used` (chaque saut vérifié de
  nos propres circuits) par clé publique ; deltas comptés au tick de
  maintenance + flush final au retrait ; `PeerStatsStore` injecté
  (pattern `GuardStore`) + `DbPeerStatsStore` (core) sur la table
  `peer_stats` (migration v16). `GET /api/ipv8/tunnel/ledger` :
  configuration, totaux et top pairs par volume.
- [x] **Étape 43. Admission par déficit sous pression** — `on_create`
  n'applique la gate que si `ledger_enforce` **et**
  `joined >= ledger_soft_cap` : admission ssi
  `served - used <= ledger_max_deficit_bytes` (le crédit de démarrage
  *est* le déficit max — un pair inconnu est toujours admis).
  `tunnel_community/ledger_*` : `enabled` (collecte, défaut on),
  `enforce` (défaut **off** — mesure expérimentale, promotion après
  validation terrain comme les guards), `soft_cap` (80),
  `max_deficit_bytes` (256 Mio). Désactivé = comportement pyipv8
  exact.
- [x] **Étape 44. `OnionbitExtCommunity` (transport)** — `community_id`
  dédié `sha1("OnionBit extension community")`, `hello` lazy vers
  pairs déjà connus (jamais de walk dédié), capabilities par
  `msg_id` + bitmap `caps`, trames `{v,...}` versionnées signées
  `ez_send` (`WIRE_EXT` : tout signé, pas de `dist`), surface fuzzée
  (`ext_packet` + régression stable). Section `ext` de la config
  (`enabled` défaut **off** — protocole observable sur le mesh,
  promotion après validation, même discipline que `messaging`) ;
  overlay visible dans `GET /api/ipv8/overlays`.
- [x] **Étape 45. Ledger bilatéral signé** — `LedgerLink` signé par
  les deux parties (`LEDGER_PROPOSE`/`SEAL`/`HEAD`/`REJECT`/`FORK`),
  doubles positions `seq_a`/`seq_b`, `tx` chiffré X25519 lisible par
  la paire seule, chaîne par `prev_*`, persistance bornée (migration
  v19), détection+preuve+gossip de fork, rejet dérive→resync,
  sign-then-serve par tranches + veto d'admission
  (`ext/ledger_enforce`, off par défaut) — bancs T5a/T5b/T5c.
- [x] **Étape 46. Curation signée** — `Attestation` auto-portante
  (curateur + signature Ed25519 sur domaine séparé) propagée par
  `ATTEST` via gossip borné ; curateurs suivis (`ext/curators`),
  stockage dedup latest-wins (migration v18), score de confiance
  **local** (`GET /api/ipv8/ext/trust/{kind}/{subject}`),
  publication `POST /api/ipv8/ext/attest`. Borne Sybil : seules les
  attestations de curateurs suivis (+ soi) sont stockées et
  re-émises. Chemin de réception durci (revue) : préfiltre curateur
  **avant** crypto, dedup avant `verify`, conflit même `ts` rejeté,
  budget `ATTEST`/émetteur, compteurs `rx/dropped/stored/tx` dans
  `GET /api/ipv8/ext`.
- [x] **Étape 47. Obfuscation de transport négociée** — mesure 47.1
  faite (ext ≈ 0,03 % de l'endpoint ; signal = contenu + cadence, pas
  volume). `msg::OBF` (8) : enveloppe `{v, blob}` = AEAD de paire
  (HKDF `ext-obf/v1`, séparé du pairbox `tx`) sur `msg_id‖len‖payload‖
  pad`, classes de taille `ext/obf_pad_bucket` (256). Capacité
  `CAP_OBF_V1` (bit 0 `hello.caps`) annoncée seulement si
  `ext/obf_enabled` ; émission enveloppée seulement vers un pair
  l'ayant annoncé — clair sinon, jamais vers legacy. Jitter
  `ext/hello_jitter_pct` (25 %). Banc T6 + fuzz `obf` dans
  `ext_packet`.
- [x] **Étape 48. Identité portable par graine (ADR-0016)** —
  graine de 32 octets racine de l'identité IPv8 : HKDF domaine-séparé
  → keypair `LibNaCLSK:` déterministe ; trois portes — phrase BIP39
  24 mots, chaîne `onionbit:<hex>`, `argon2id(pseudo‖mdp)` (RFC 9106
  premier jeu, sel = SHA-256(domaine‖pseudo)). `identity_seed.bin`
  fait foi (`ipv8_keypair.bin` = cache, clé seule = legacy sans
  phrase) ; `GET /api/identity`, `GET /api/identity/recovery`
  (secret, action explicite), `POST /api/identity/restore`
  (`restart_required`) derrière `api_key_auth` ; section « Identité »
  des réglages Flutter (adresse, phrase avec avertissement,
  restauration 3 onglets).

---

## Notes de suivi

Ajouter ici, au fil de l'avancement, tout écart constaté par rapport au
plan initial (dépendance qui ne convient pas, étape scindée en deux,
risque IPv8 sous/sur-estimé, etc.), avec la date.

- 2026-10-06 (ADR-0017 — **transport furtif**, statut Proposée) :
  analyse complète de la furtivité anti-censure — décision
  structurante : indétectabilité et interopérabilité legacy Tribler
  s'excluent sur un même nœud (le walk legacy trahit toujours) ;
  le mode furtif serait donc un mode dédié OnionBit↔OnionBit avec
  bootstrap hors-bande (liens d'invitation type bridges Tor),
  trames indiscernables de bruit (obfs4-style), tunnel+messagerie
  conservés au-dessus, BitTorrent public exclu. Aucun code engagé.
- 2026-10-06 (ADR-0015 — **boucle de curation exploitée**) : le score
  de confiance local sort du diagnostic — pastille ±n sur les
  résultats de recherche, « Approuver »/« Signaler » au clic droit
  (dialogue pré-rempli), « suivre ce curateur » depuis les
  attestations stockées. Requête unique par sujet (pas de polling
  N+1) ; score local, aucun blocage automatique.
- 2026-10-06 (UI mécanismes OnionBit complets) : section « OnionBit »
  des Réglages (ext enabled/ledger/obf/curators — restart ; ledger
  collect/enforce tunnel — hot) + onglet Diagnostic enrichi (registre
  bilatéral, attestations + publication signée depuis l'UI).
- 2026-10-06 (UI Diagnostic — onglet « OnionBit ») : `caps_names`
  exposé à l'utilisateur — état local de la communauté ext et pairs
  OnionBit reconnus avec leurs capacités en pastilles.
- 2026-10-06 (ADR-0015/ADR-0011 — `CAP_MSG_V1`, bit 1 de
  `hello.caps`) : pont de découverte entre ext et la messagerie
  anonyme. Le service reste dans `onionbit-tunnel` (plan de données —
  circuits + lanes e2e chiffrées) ; ext n'annonce que la capacité
  dans son `hello` — deux installs OnionBit savent qu'une liaison
  messagerie est tuable sans tentative aveugle. Annoncé seulement si
  le service est réellement démarré (`messaging_enabled` + tunnel
  actif) ; `messaging_enabled` est déjà `true` par défaut dans la
  config tunnel — les installs par défaut l'annoncent donc. API :
  `caps_names` décodé dans `GET /api/ipv8/ext` (local + par pair).
- 2026-10-06 (ADR-0015 — `ext.enabled` **on par défaut**) : écart au
  plan initial qui prévoyait l'activation « après validation ». La
  validation terrain étant verte (T1 + soak + bancs interop), le
  défaut devient `true` : sinon deux installs par défaut ne se
  reconnaissent jamais comme OnionBit (démontré par
  `bench_ext_interconnect.ps1` — phase off : IPv8 ok mais
  `peer_count=0` ; phase on : `peer_count=1` en ~7 s). Le HELLO signé
  en clair suit la même discipline que les `introduction-request`
  IPv8 legacy. `Ipv8Config::default()` reste neutre (off) ;
  `enabled=false` honoré. Bancs de mesure figés sur la surface
  legacy (`fingerprint_mesh` baseline, `sec_leak_capture`) via
  `enabled=false` explicite. Campagne post-merge par ailleurs verte :
  `verify_all` complet, migration v13→v19 sur base réelle (données
  intactes, purge catalogue v14 intentionnelle), interop 4/4 +
  Tribler.exe relay/download + hidden download/seed/killseeder +
  messaging E2E 19/19. `hidden_py2py`/`public_dht` : le swarm public
  Tribler.exe ne répond pas depuis la machine de banc
  (`DispersyBootstrapper` ne produit aucun pair — infrastructure
  dispersy dormante ; le daemon OnionBit bootstrappé sur le même
  réseau voit lui les pairs publics — environnement, pas protocole).
- 2026-10-06 (ADR-0015 validation terrain — **vert**) : soak réel
  `bench_ext_ledger_soak.ps1` (4 daemons ext+OBF, mesh fermé,
  speedtest tunnel 2 sauts — 6,1 Mio relayés) : 10/10 — convergence
  ext, liens scellés des deux côtés (hash partagé), `forks=0`, OBF
  38/38. Deux défauts trouvés et corrigés : (a) `Network.services`
  partagée perdait la marque `EXT` sous `remove_peer_key`
  (churn/éviction DHT) → silence ledger persistant — `ext_peers`
  fait désormais foi (`ExtPeer.addr` = dernier hello,
  `ext_targets()` re-guérit l'annuaire), régression T7 ; (b)
  `DbLedgerStore` indexait `(pk_b, seq_b)` dès la proposition non
  scellée → retry post-REJECT = faux fork — évaluée au sceau
  seulement, parité `InMemoryLedgerStore`. T1 silence legacy vert
  (oracle : `Need a DHT provider` = bruit interne Tribler, DHT
  fermée). Le REJECT-resync observé est nominal : reliquat au-delà
  de la mesure du pair non réglable, gate conservée.
- 2026-10-06 (diagnostic des connexions) : nouvel onglet « Connexions »
  de la page Diagnostic (12 onglets) adossé à l'extension Rust
  `GET /api/connections` — agrégat par adresse distante `ip:port` des
  rôles observés (pair IPv8 + overlays, noeud DHT, flags tunnel,
  cibles des sockets de sortie via `TunnelCommunity::exit_sources`,
  pairs BitTorrent `conn_kind` tcp/uTP/socks) + sockets d'écoute
  locales (`ipv8-udp`, `bittorrent`, `socks5` des lanes,
  `tunnel-exit-udp`). Contrat documenté dans
  `docs/reference_tribler/api_rest_mapping.md`.
- 2026-10-02 (interface web — Phase 7 exécutée) : étapes 31–35
  implémentées et validées (`verify_all` vert : workspace Rust +
  i18n + analyze/test/build web Flutter ; fumée réelle sur daemon
  — `/` 200, fallback SPA 200, `/api/*` 401 sans clé, SSE streamé).
  Décision same-origin consignée en ADR-0012. Écart du plan initial :
  `ServeDir` + fallback maison plutôt que `rust-embed` (build externe,
  `dist/web/` auto-détecté), et `fetch_client` retenu pour le
  transport navigateur. Suite au premier essai : clé API **injectée**
  dans `index.html` servi (`api/web_ui_inject_key`, meta
  `onionbit-api-key`) + lanceur `OnionBit Web.cmd` qui démarre le
  daemon s'il ne tourne pas — parcours identique au desktop.
  **Reste manuel** : la passe UX multi-navigateurs de l'étape 35
  (drop HTML5, player `/stream`, notifications) est à confirmer à la
  main — les éléments automatisables sont verts.
- 2026-10-02 (validation guards) : matrice interop réelle exécutée
  via `-Guards` sur `interop_hidden_tribler_{download,seed}.ps1` —
  sens A (Tribler←OnionBit) et sens B (OnionBit←Tribler) verts en
  hops 1, 2 et 3 (SHA-256 exact, premiers hops des circuits
  multi-hop ⊆ guard set). Deux défauts trouvés et corrigés : self dans
  le pool de candidats (auto-adoption guard via `rp_info` nous
  élisant RP) et bypass guards des circuits RP via `pick_first_hop`.
  Persistance redémarrage vérifiée sur réseau public réel (set 3
  actifs + 2 réserve rechargé à l'identique). Baseline sans guards
  exécutée (`interop-noguards-B-h1`) : `enabled=false`, set vide,
  premier hop libre (`beb1d983…`) — tirage pyipv8 inchangé. Reste :
  activation par défaut toujours différée.
- 2026-10-02 (test public guards — **vert**) : daemon de banc sur le
  réseau réel (`target/guards-public-20261002-1852/`, bootstrappeurs
  par défaut, `guards_enabled=true`), download anonyme Big Buck
  Bunny (`dd8255ec…`, magnet WebTorrent canonique). Critères :
  **octets vérifiés > 0** — 276,4 Mo, 100 % en ~10 min (pic 1,2
  Mo/s, ~450 Ko/s moyen sur tunnels) ; **route publique effective**
  — exits et guards réels (24.87.16.68, 172.59.188.242,
  119.213.229.4, 95.19.54.167, 109.221.82.110) ; **guard set non
  vide** — 3 actifs + 2 réserve adoptés 18:58:04, `failures=0`,
  persistés en table `guards` ; **premier hop ∈ GuardSet** — les 8
  circuits DATA 2-sauts post-adoption pinnet tous le guard
  `30802970…` (sticky conforme), données transportées dessus (pas un
  circuit proactif parallèle). Absence de fallback direct : kill
  switch engagé à la création de chaque lane (18:57:37 lane 1 saut,
  19:01:10 lane 2 sauts), désarmé à READY. Notes : circuits 1-saut =
  premier hop = exit `EXIT_BT`, guards non applicables (assertion
  multi-hop, comme la matrice interop) ; les circuits créés avant
  l'adoption gardent leur premier hop libre — attendu, le critère
  s'évalue post-adoption.
- 2026-10-02 (décision guards par défaut) : `guards_enabled=true` —
  conditions (3)+(4) remplies : download public vert ci-dessus +
  validation workspace complète verte (check/clippy/fmt/tests Rust
  + i18n + analyze/test/build web Flutter). Garde-fous en place :
  désactivation à chaud (`POST /api/settings` + `set_enabled` live,
  sans redémarrage), toggle UI « Nœuds guards », API
  `GET /ipv8/tunnel/guards` lecture seule, warn
  `guards_pool_etroit` quand le pool est insuffisant, migration sûre
  (absence de clé → nouveau défaut ; `false` explicite préservé).
  Communication : mesure expérimentale de réduction d'exposition
  Sybil, jamais garantie d'anonymat.
- 2026-10-03 (migration des défauts gelés) : `config_version`
  estampille le schéma de `configuration.json` (extension Rust —
  `TriblerConfig` n'a pas de marqueur). Un fichier legacy (v0) voit
  réalignée sur le défaut actuel toute valeur **encore égale à
  l'ancien défaut** des interrupteurs « Tunnels anonymes » :
  `guards_enabled` `false`→`true` (seul glissement réel —
  `enabled`/`exitnode_enabled` n'ont jamais changé de défaut, une
  valeur non défaut y est un choix explicite préservé). Fichier
  réécrit estampillé une fois : la migration ne rejoue pas, un choix
  posé après coup vers l'ancienne valeur est honoré ;
  `config_version` non patchable via `POST /api/settings`.
- 2026-10-02 (plafond de relais) : `tunnel_community/max_joined_circuits`
  exposé (défaut 100 = `should_join_circuit` pyipv8) — borne la charge
  de relais imposée par le réseau, configurable dans l'UI
  (« Relais servis maximum », restart-only comme ses voisins).
- 2026-10-02 (plafond de débit servi) : `tunnel_community/max_relayed_rate`
  en mode **auto par défaut** (`-1` : `bandwidth/share` = 1/3 de la
  capacité upload mesurée — UPnP WAN `GetLinkLayerMaxBitRates`, sondes
  POST opt-in, pic passif endpoint ; `0` = illimité, `>0` = fixe —
  extension Rust, pyipv8 sert sans plafond) —
  seau à jetons (rafale 1 s) sur la pompe d'émission sérialisée,
  couvre cellules relayées **et** datagrammes de sortie ; l'excédent
  est perdu (perte UDP, lissée par uTP aux extrémités). Appliqué à
  chaud (`set_relay_rate_bps`), UI « Débit servi maximum » en Kio/s.
- 2026-10-02 (tempête ping/pong + discipline DHT anonyme) : le
  fingerprint majeur du mesh v2 (~6 400 `on_cell`/s symétriques,
  ~365 Ko/s par sens sur un magnet en stall, ~200× Tribler) avait
  une **cause racine protocolaire** : `TunnelPong` était un alias de
  `TunnelPing` — la réponse au keepalive de circuit repartait
  étiquetée `ping`, et chaque extrémité répondait à ce « ping » par
  un autre ping → boucle auto-entretenue à ~6 000 cellules/s dès le
  premier keepalive entre deux nœuds OnionBit (les pairs pyipv8
  répondent un vrai pong : l'orage n'apparaît qu'en mesh natif).
  Correction : vrai type `TunnelPong` (`msg_id=7`) + test de
  non-boucle. Vérifié en mesh : **~1 cellule/s résiduelle** sur le
  même scénario (vs ~6 100/s avant). Durcissement DHT conservé
  (mesures préventives + amplification théorique coupée) : bug
  vendored de double envoi corrigé, posture client-only par défaut
  (`anon_dht_client_only`), budget de requêtes entrantes
  (`inbound_queries_per_second`), plafond de débit socket
  (`anon_dht_rate_pps` = 30), backoff exponentiel jitteré des
  re-lookups sans progrès (`anon_dht_backoff_cap_secs` = 900, état
  « discovery dégradée » à ≥3 passes idle), conntrack de sortie
  (`exit_inbound_source_ttl_secs` = 300, table
  `exit_inbound_max_sources` = 2048 — non-IPv8 seulement, l'e2e
  hidden-service exempté). Tests : `dht_backoff.rs` (backoff math,
  budget inbound mesuré, décroissance de cadence), loopback
  (client-only, rafale, conntrack, non-boucle ping/pong).
- 2026-10-02 (critères de sortie arrêtés) : séquence post-campagne
  figée — (1) clôture journal fuzz, (2) fingerprint Tribler en mesh
  contrôlé **et** réseau public (durée/rôle/charge/métriques alignés
  avec les sessions OnionBit), (3) **test public guards** : ligne
  verte seulement si `octets vérifiés > 0 ∧ route publique effective
  ∧ guard set non vide ∧ premier hop ∈ GuardSet` — le guard doit
  porter le circuit qui transporte les données, pas un circuit
  proactif parallèle ; à consigner : MIDs des guards en début de run,
  premier hop et route observés, exit public, compteurs BitTorrent
  vérifiés, absence de fallback direct, durée/retries, état du set
  final. (4) validation workspace complète, (5) décision
  `guards_enabled` par défaut — seulement après (3)+(4) verts, avec
  désactivation de diagnostic, migration sûre des profils, lecture
  seule API, logs de pool trop petit, et communication « mesure
  expérimentale de réduction d'exposition Sybil », jamais une
  garantie d'anonymat. Interdictions pendant la campagne : pas de
  modification logique tunnel, targets fuzz ou dépendances
  (provenance des lignes du journal). Messagerie : ADR-0011 **acceptée** —
  Phase 8 (étapes 36-41), tests de protocole catalogués en
- 2026-10-04 : parité réseau et ports UDP/BT/NAT — (1) ports UDP IPv8 :
  remplacement du repli direct `0.0.0.0:0` par `bind_dual_with_retry`
  (`MAX_PORT_RETRY_ATTEMPTS = 1000`, incrémentation `port + 1` en collision,
  fidèle à `create_socket_with_retry` de Tribler) ; (2) ports BitTorrent :
  pré-sélection de la plage standard `6881..=6891` quand le port vaut `0`
  (parité `ltsession.listen_on(port, port + 10)`) ; (3) NAT-PMP (RFC 6886) :
  implémentation native dans `onionbit-bittorrent::natpmp` pour l'ouverture
  TCP/UDP en session claire avec renouvellement et libération propre à l'arrêt.
- 2026-10-02 : campagne libFuzzer de référence terminée — 6/6
  cibles, 6,28 Md d'exécutions, 0 crash/timeout/OOM
  (`docs/security/fuzz_journal.md`). Guard nodes expérimentaux
  implémentés (ADR-0010, `guards_enabled=false` par défaut) :
  `GuardSet` + persistance SQLite v11 via `GuardStore`/`DbGuardStore`,
  `GET /api/ipv8/tunnel/guards`, CLI `tunnel --show`. Extension
  observabilité : `GET /api/ipv8/tunnel/debug/circuit-downloads`
  (corrélation circuits↔downloads anonymes, sans équivalent pyipv8).
  Restes : rotation guards au watchdog, validation terrain avant
  activation par défaut, campagne longue sur les cibles à fort
  rendement, campagne Linux/ASan complémentaire.
- 2026-10-01 : onglet Diagnostic « Statistiques » enrichi — sections
  Daemon (version, **uptime** nouveau champ `uptime_sec`, taille DB,
  espace disque du dossier de réception), Contenu (torrents connus,
  downloads actifs/en pause/en échec), Réseau IPv8 (pairs, trafic
  overlay `total_up`/`total_down`, trafic BitTorrent session —
  `libtorrent.total_{recv,sent}_bytes` désormais renseignés en sommant
  les stats moteur), Anonymat (sessions, lanes `socks5_sessions`,
  circuits DATA prêts par lane, sorties actives). Le compteur
  « Canaux » (`metadata_type=400`, structurellement à 0 — les canaux
  GigaChannel ne circulent plus sur le réseau) est retiré de l'UI ;
  le champ `num_channels` reste émis par l'API.

- 2026-09-28 : phase 5b planifiée (étapes 21-28) — parité complète de
  l'API de contrôle dans le daemon, d'après l'inventaire
  `api_endpoints_complet.md`. Constat clé : la `DhtCommunity` de
  `onionbit-ipv8` n'est pas instanciée dans `Ipv8Stack` (pré-requis de
  tout `/api/ipv8/dht/*` et de `tunnel/peers/dht`) et aucune
  configuration n'est persistée (`configuration.json` absent → lot 1
  en fondation). Arbitrages actés : clé API en parité Python même en
  loopback, `asyncio/*` adapté à tokio, `identity/*` exclu (ADR),
  items RSS persistés. Constats résiduels à traiter dans les étapes :
  `file_priority`/`queue_position`/`auto_managed` sans équivalent
  rqbit, `tracker_force_announce`/`DELETE trackers` selon surface
  rqbit, `get_pieces`/`get_availability` idem.
- 2026-09-27 : durcissement des bancs d'interop. `verify_packets.py`
  exige désormais une whitelist de `msg_id` (`--allow-msg-id`) —
  premier run : détection d'un `similarity-request` pyipv8 (msg_id=1)
  que l'ancien total global absorbait ; whitelist `interop_ipv8.ps1` =
  `1,2,3,4,246,245,250,249` (famille DiscoveryCommunity complète) —
  et compte des statistiques séparées par `msg_id`/type
  (signé/non-signé/invalide). Suppression des dumps de secrets de
  session : `KEYS|` (clés+sels) retiré de `py_tunnel_node.py` et des
  exemples tunnel Rust, accesseur `debug_session_keys` supprimé ;
  `KEY|` (clé publique, redondante avec les fichiers `--key-file`)
  retiré de `py_dht_node.py`, `dht_interop_node` et
  `discovery_interop_node`. `interop_tribler.ps1` : ports fixes
  22090/23100/22091 remplacés par tirage de ports libres (UDP×2
  distincts + TCP) — plus de collision avec un Tribler local ou un
  reste de run. Les quatre bancs (ipv8, dht, discovery, tunnel)
  + `interop_tribler.ps1` restent verts après le durcissement.
- 2026-09-27 : étape 0 terminée. Découverte de `librqbit` (ADR-0001) qui
  réduit fortement le risque des phases 1-2 par rapport à l'hypothèse
  initiale d'un moteur BitTorrent écrit entièrement à la main.
- 2026-09-27 : étape 6 terminée. Correction de fidélité : l'endpoint
  `/api/events` Python est du **SSE** (`text/event-stream`), pas un
  WebSocket — `onionbit-api` reproduit ce format exact. Écarts DTO
  connus consignés dans `api_rest_mapping.md` (`eta` en chaîne
  formatée, `num_seeds`/`num_connected_seeds` à 0 tant que le scraping
  trackers n'est pas implémenté).
- 2026-09-27 : étapes 1 et 2 terminées. Deux corrections de fidélité par
  rapport au plan initial : (a) le chiffrement de tunnel IPv8 est
  **ChaCha20-Poly1305** et non AES-GCM (source : `ipv8-rust-tunnels`
  `crypto.rs`) ; (b) bencode est implémenté maison dans `onionbit-format`
  plutôt que via `librqbit-bencode`, pour garder le parsing borné sous
  notre contrôle (et éviter une dépendance pour un codec simple).
- 2026-09-27 : nouvelle ressource d'interop documentée — Tribler 8.4.3
  installé (`<Tribler install dir> (env `TRIBLER_EXE`)`) : `Tribler.exe` = noeud
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
