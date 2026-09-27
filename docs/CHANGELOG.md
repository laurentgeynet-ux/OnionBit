# Changelog — étapes franchies

Format : une entrée par étape de `docs/plans/roadmap.md`, la plus récente
en haut.

## Étape 10 — interop DHT Rust ↔ pyipv8 prouvée (2026-09-27)

- **Correctif filaire `Packet`** : pyipv8 n'insère
  `GlobalTimeDistributionPayload` (`dist`, 8 octets) que dans les
  paquets construits par `create_introduction_*`/`create_puncture*` —
  intros/punctures signées `246/245/234/233/249/231` (+ non signées
  `250/232`, déjà couvertes). Les messages `ez_send` (DHT 1-10,
  cellules tunnel, content-discovery) sont `[auth, payload]` **sans
  `dist`**. `Packet::parse` ne lit `global_time` que pour
  `DIST_MSG_IDS` ; nouveaux `Packet::sign_no_dist` (layout `ez_send`)
  et `sign_auto` (choix par `msg_id`). Sans cela, chaque paquet DHT
  Python etait desaligne de 8 octets → `type d'adresse inconnu`, et
  reciproquement les paquets Rust etaient rejetes par Python.
  Confirmé par décodage octet-par-octet d'une capture
  (`identifier/lan_address/target` exacts).
- **Émetteurs corrigés** : `DhtCommunity::send`/`reply` → `sign_auto`
  (les intros 246/245 que le DHT reutilise gardent `dist`),
  `ContentDiscoveryCommunity::send_payload` → `sign_no_dist`,
  `TunnelCommunity::send_destroy` → `sign_no_dist` (`send_destroy`
  pyipv8 = `ezr_pack` sans dist).
- **Banc d'interop** : `scripts/interop_dht.ps1` +
  `crates/tribler-ipv8/examples/dht_interop_node.rs` +
  `scripts/interop/py_dht_node.py` (vrai `DHTCommunity` pyipv8,
  loopback 127.0.0.1:12100↔12101). Résultat **`INTEROP DHT OK`** —
  les 10 assertions passent :
  - Python→Rust : `find_values` (token), `store_value` **signé**
    accepté, relecture avec signature vérifiée (`pubkey` non nul),
    store à token bidon rejeté, lecture de la valeur signée Rust
    (signature vérifiée par Python).
  - Rust→Python : `find_values` (token), `store_value` signé accepté,
    **rotation des secrets Python** au premier store → token évincé
    rejeté (`RUST_STALE_REJECTED`), token frais accepté
    (`RUST_REFRESHED_STORE_OK`).
- Diagnostic : `msg_id` ajouté au log d'erreur des handlers
  d'`UdpEndpoint` ; tap brut datagrammes dans l'exemple (déjà utilisé
  par les bancs tunnel).
- **`UdpEndpoint::run` résilient** : `recv_from` ne tue plus la boucle
  d'écoute (`WSAECONNRESET` Windows après ICMP « port injoignable »
  d'un envoi vers un pair mort rendait le noeud sourd — flaky tests
  loopback) : erreur loguée `warn!` + pause 10 ms + poursuite.
- **Flake `content_discovery` corrigé** : `gossip_tick` absorbait le
  premier tick immediat de `tokio::interval` a un instant non
  deterministe et pouvait emettre un `HealthPayload` en doublon des
  qu'un pair etait verifie → egalite stricte de compteur cassante.
  Tick immediat consomme + payload vide jamais emis (coherent avec le
  handler `HEALTH_REQUEST`) + tests sur intervalle long.
- **`verify_all.ps1` durci** : `$ErrorActionPreference` n'intercepte
  pas les codes de sortie natifs — chaque etape verifie
  `$LASTEXITCODE` et echoue immediatement (un `cargo test` rouge ne
  peut plus etre masque par « Validation complete OK »).

## Régression — `anon_hops` câblé dans PUT/PATCH `/api/downloads` (commit `718b2b8`)

- `PUT /api/downloads` refusait `anon_hops > 0` alors que la session
  supportait déjà les lanes anonymes — routage vers
  `add_download_anon`/`add_torrent_bytes_anon`, validation
  safe-seeding (sémantique Tribler) et stack IPv8 active.
- `PATCH /api/downloads/{ih}` : `anon_hops` seul accepté —
  `update_hops` détruit l'engine, recrée sur la nouvelle lane,
  restaure l'état (pause, trackers), rollback best-effort en cas
  d'échec, persistance DB (`downloads.anon_hops`, migration v2).
- Réponses GET : `hops`/`anon_download` réels (plus codés à 0/false).
- 22 tests API verts dont régression anonyme.

## Étapes 17-18 — packaging desktop + modèle mobile (2026-09-27)

- **`scripts/build_release.ps1`** : build release reproductible
  (cible hôte ou `-Target`), sortie `dist/<target>/` +
  `build-manifest.json` (version, commit, rustc, date UTC). Matrice
  prévue : windows-x64, windows-arm64, linux-x64, macos-arm64.
- **Windows x64 vérifié** : release build OK, smoke test du binaire
  (API loopback + `PUT /api/shutdown` propres). Autres cibles non
  vérifiables sur cette machine (toolchain MSVC ARM64, cross-gcc
  Linux, SDK Apple absents) → CI matricielle requise.
- **Étape 18 documentée** (`docs/plans/mobile_execution_model.md`) :
  Android/iOS imposent un service de premier plan, pas de daemon —
  façade FFI `tribler-mobile` à créer (étape 19), anonymat off par
  défaut, `pause_all`/`resume_all` à ajouter. Verdict : compilable,
  comportement adapté (pas de seeding permanent ni d'exit node).

## Étape 16 — durcissement + tests de bout en bout (2026-09-27)

- **`tribler-test-support` peuplé** : `test_torrent_bytes(name, len)`,
  `free_port()`, `wait_for(timeout, f)` — fixtures dupliquées dans
  `tribler-api`/`tribler-cli`/`tribler-daemon` remplacées par la
  fixture partagée.
- **Téléchargement loopback réel** (`tribler-bittorrent/tests/
  loopback_download.rs`) : seeder + downloader rqbit uTP en loopback,
  `initial_peers`, contenu vérifié octet-pour-octet.
- **Persistance/redémarrage** (`tribler-core/tests/lifecycle.rs`) :
  deux `CoreSession` successives sur le même `state_dir` — le
  téléchargement est restauré (info-hash + état pause) ; un download
  supprimé n'est pas restauré.
- **Migration de schéma** (`tribler-db/tests/migrations.rs`) : base
  figée à v1 migrée vers `SCHEMA_VERSION` à l'ouverture avec
  conservation des données ; réouverture idempotente ; refus
  `SchemaTooNew` si la base est plus récente.
- **Revue de sécurité** `docs/security/revue_garde_fous.md` :
  inventaire anti-SSRF / exit policy / kill switch / proxy guard /
  hidden seeding + le test qui couvre chaque garde-fou.

## Documentation — inventaire exhaustif de l'API web (2026-09-27)

- `docs/reference_tribler/api_endpoints_complet.md` créé : recensement
  complet des fonctions exposées à l'interface web Tribler (67 routes
  `/api/*` + sous-endpoints `/api/ipv8/*` + `/ui` + `/docs`), avec pour
  chacune la méthode, le chemin, la description, les paramètres et leurs
  valeurs par défaut/bornes min-max, l'emplacement d'implantation Python
  (`D:\Projet\Tribler_sources\tribler`) et le pendant Rust
  (`crates/tribler-api`) avec statut de portage.
- Inclut l'arbre de configuration complet servi par
  `GET /api/settings` (défauts `tribler_config.py` + `ipv8/configuration.py`
  + `TunnelSettings`) et les topics SSE de `/api/events`.
- Écarts identifiés : routes Python non portées (`events/info`,
  `downloads/clierrors`, `default_trackers`, `tracker_force_announce`,
  panneau IPv8 `asyncio`/`dht`/`identity`/`isolation`/`network`/
  `noblockdht`, speedtests de circuits…), signatures divergentes
  (`dirspace` PUT→GET, `hop`→`session`), endpoint `/api/recommender/clicked`
  appelé par l'UI sans backend Python.

## Étape 15 — parité API REST/SSE (2026-09-27)

Couverture complète des endpoints `tribler.core.restapi` utiles au
futur client, avec les ajouts d'infrastructure nécessaires.

- **Stack IPv8 dans `CoreSession`** (`tribler-core/src/ipv8_stack.rs`) :
  endpoint UDP, `Network`, `DiscoveryCommunity`, `ContentDiscoveryCommunity`
  (provider = base `channel_node` + sérialiseur mdblob signé),
  `TunnelCommunity` optionnelle + serveur SOCKS5 par lane anonyme et
  moteur `BtEngine` dédié par nombre de sauts (`anon_engine(hops)`).
  Reglages dans `CoreConfig.ipv8` (`enabled`, `listen_addr`,
  `bootstrap_peers`, `enable_anonymity`, `peer_flags`,
  `tribler_tunnel_community`).
- **`downloads.anon_hops`** (migration DB v2) : le téléchargement est
  routé vers la lane anonyme correspondante (`anon_hops` de
  `PUT /api/downloads`) et restauré sur la bonne lane au démarrage.
- **`tribler-format::mdblob::encode_entry`** : sérialisation signée
  `.mdblob` (réponses du remote-select).
- **Endpoints ajoutés** (36 routes au total) : `downloads/{ih}/torrent`
  `trackers` (GET/PUT) `files` `stream/{i}` (seek par `start`),
  `settings` GET/POST, `shutdown`, `statistics/tribler|ipv8|dirspace`,
  `metadata/torrents/{ih}/health` (+`refresh=1` via checker) `popular`
  `health` `search/local|completions|vocabulary` `torrents/{ih}/tags`
  (PUT/DELETE/PATCH), `search/remote`, `torrentinfo/uri|file`,
  `createtorrent` (+`dryrun`, via `librqbit::create_torrent`),
  `libtorrent/settings|session` (par lane), `ipv8/overlays` +
  `ipv8/tunnel/{settings,circuits,relays,exits,swarms,peers}`,
  `files/browse|list|create`, `rss`, `versioning/*`, `logging`.
- **`tribler-bittorrent`** : `Download` expose `files()`/`trackers()`/
  `add_tracker()`/`torrent_bytes()`/`stream_file_from()` (seek) ;
  trackers additionnels partagés par info-hash au niveau `BtEngine`.
- **`tribler-tunnel`/`tribler-ipv8`** : accesseurs de stats
  (`circuits_info`, `relays_info`, `exits_info`, `swarms_info`,
  `tunnel_peers_info`, compteurs d'octets `UdpEndpoint`).
- Reglages mutables à chaud (`rss.urls`, `watch_folder`) reflétés par
  `effective_config()` ; 13 nouveaux tests d'intégration HTTP.
- Mapping complet et écarts assumés :
  `docs/reference_tribler/api_rest_mapping.md`.

## Étape 14 — services secondaires : content discovery, torrent checker, RSS, watch folder (2026-09-27)

Quatre services inspirés de Tribler (`src/tribler/core/content_discovery/`,
`torrent_checker/`, `rss/`, `watch_folder/`), câblés dans `CoreSession`
via `CoreConfig`.

- `tribler-ipv8::content_discovery` : community `9aca62f8…1648` —
  payloads santé (msgs 3/4, `HealthInfo` binaire pyipv8), version
  (101/102) et remote-select (201/202) ; trait `ContentProvider`
  injecté par la couche supérieure ; gossip périodique des santés
  vers les pairs de la community. Tests loopback `tests/content_discovery.rs`.
- `tribler-core::services::torrent_checker` : scrape BEP-15 UDP
  (connect/announce→scrape) et HTTP bencode (`files` dict), socket UDP
  dédiée partagée, sélection des torrents les moins récemment vérifiés
  depuis `torrent_state`, persistance seeders/leechers/last_check +
  notification `TorrentHealthUpdated`. `IpPolicy` appliquée aux
  trackers. Test `torrent_checker_udp_scrape` (tracker factice
  loopback).
- `tribler-core::services::rss` : watchers périodiques (`RssManager`),
  extraction des URLs `.torrent` du XML, requêtes conditionnelles
  (ETag/Last-Modified) et backoff `Keep-Alive: timeout=N`, fetch des
  torrents via le helper anti-SSRF partagé, notification
  `TorrentMetadataCreated`. Test `rss_discovers_torrent_and_notifies`.
- `tribler-core::services::watch_folder` : scan récursif périodique,
  `.torrent` et `.magnet`, dédup par chemin+hash, import via
  `CoreSession::add_download`. Test `watch_folder_imports_torrent`
  (`.torrent` — le magnet nécessite le DHT, hors champ offline).
- `CoreConfig` : `watch_folder_dir`, `watch_folder_interval_ms`,
  `rss_urls`, `rss_interval_ms`, `enable_torrent_checker`,
  `torrent_checker_interval_ms` (tous désactivés par défaut).
- `CoreSession` : démarre les services configurés dans `start`/
  `start_offline`, les arrête dans `stop()` (pas de tâche orpheline),
  expose `torrent_checker()`/`rss()` pour l'API future.
- `Notification::TorrentHealthUpdated` → event SSE
  `torrent_health_updated` dans `tribler-api`.

## Étape 13 — `tribler-network-policy` : anti-SSRF, exit policy, kill switch (2026-09-27)

Crate de politiques réseau pures (sans dépendance vers ipv8/bittorrent)
+ intégration dans tunnel, bittorrent et core.

- `address_policy::IpPolicy` : anti-SSRF — catégories refusables
  (loopback, privé RFC1918 + CGNAT + ULA, link-local, multicast,
  unspecified, réservé/documentation, IPv4-mapped IPv6), ports bornables.
- `exit_policy` : port fidèle de `DataChecker`/`is_allowed` (pyipv8
  `exit_socket.py`) — `could_be_utp`/`udp_tracker`/`dht`/`ipv8`,
  `is_exit_data_allowed` exige `PEER_FLAG_EXIT_BT`/`PEER_FLAG_EXIT_IPV8`
  ou le préfixe de la community. Les constantes `PEER_FLAG_*` vivent
  ici désormais (source unique, ré-exportées par `tribler-tunnel::routing`).
- `kill_switch::KillSwitch` : atomic bool + raison diagnostic + `guard()`.
- `proxy_guard::validate_local_socks5_url` : `socks5://`/`socks5h://`
  numérique loopback uniquement (ni DNS, ni credentials, ni port nul).
- `tribler-tunnel` : `exit_data` ET `exit_recv_data` appliquent
  `is_exit_data_allowed` (les deux sens, comme `sendto` +
  `datagram_received` côté pyipv8) ; les flags de sortie = `peer_flags`
  locaux annoncés. Test `tunnel_exit_drops_non_bt_or_unflagged` ;
  les sorties des tests existants portent désormais `EXIT_BT` et des
  payloads uTP-shaped.
- `tribler-bittorrent` : `BtEngine::start` valide `socks5_proxy`
  (distant = échec de démarrage, jamais de repli direct) ; watchdog
  TCP du proxy + `KillSwitch` qui bloque `add`/`resume` tant que le
  proxy est injoignable ; `kill_switch()` exposée pour le futur
  câblage tunnel (circuits morts). Tests `tests/policy.rs`.
- `tribler-core` : `CoreConfig.ip_policy` (stricte par défaut,
  permissive offline) appliquée dans `Session::add_download` aux URI
  `http(s)` — résolution DNS puis refus fermé sur toute adresse niée.
  Tests `tests/policy.rs`.

## Étape 12 (jalon) — suivi des flags + interop Tribler 8.4.3 installé (2026-09-27)

L'etape 12 est close : les deux items restants du roadmap sont
valides.

- **Suivi des flags de service via la decouverte**
  (`community.rs`) : handlers `introduction-request`/`response`
  (anciens msgs 246/245 et nouveaux 234/233) sur le prefixe tunnel ;
  `extra_bytes` = bitmask `>H` (`ExtraIntroductionPayload.flags`,
  packer `Flags` pyipv8). `flag_registry` = `candidates` Python :
  `get_candidates(flag)` filtre les pairs par flags annonces, les
  candidats `created`/`extended` marquent les vraies sorties
  (`ANY_EXIT_FLAGS`), `send_introduction_request` publie nos flags.
  Test `tunnel_introduction_tracks_exit_flags`.
- **`community_id` parametrable** : `TunnelCommunity::new_with_id` +
  `TRIBLER_TUNNEL_COMMUNITY_ID` (`a3591a6b…d6bc`, prefixe de
  `TriblerTunnelCommunity` — distinct du `81ded073…c9f3` pyipv8).
- **Interop contre Tribler 8.4.3 installe** (`scripts/interop_tribler.ps1`,
  `examples/tribler_relay_interop.rs`) : `Tribler.exe -s` en etat
  isole (`TSTATEDIR` + `CORE_API_PORT`/`CORE_API_KEY`, config
  pre-ecrite sans BOM, bootstrappeurs vides — aucun trafic externe).
  Resultat valide : flags Tribler appris par introduction (`9` =
  RELAY|SPEED_TEST), `create`→`created`, circuit 2 sauts
  Rust→Tribler(relais)→Rust(sortie) et echo uTP 20 octets de bout en
  bout. Tribler relaie mais ne sort pas (`exitnode_enabled` non
  exposable par config — par conception).

## Étape 12 (correctif) — `perform_http_request` : assemblage strict + pas de fuite (2026-09-27)

- `community.rs::perform_http_request` : le `total` est fige au
  premier chunk recu (chunks incoherents ignores), l'assemblage exige
  la contiguite `0..total` (un trou = timeout, plus de reponse
  partielle silencieuse), et l'entree `http_requests` est retiree sur
  TOUS les chemins (succes, timeout, erreur d'envoi/circuit) — avant,
  les erreurs precoces fuyaient l'entree.

## Étape 12 (jalon) — interop tunnels Rust↔pyipv8 validée (2026-09-27)

L'objection « tout le chiffrement peut diverger » est levee pour le
plan de donnees des tunnels : `scripts/interop_tunnel.ps1` fait
converser le vrai `TunnelCommunity` pyipv8 (venv interop) avec notre
`tribler-tunnel` en loopback.

- `scripts/interop/py_tunnel_node.py` : noeud `TunnelCommunity`
  (flags RELAY|EXIT_IPV8|EXIT_BT) + echo UDP + dump des
  `SessionKeys` ; exporte sa cle publique via keyfile.
- `crates/tribler-tunnel/examples/tunnel_interop_node.rs` : noeud
  Rust qui cree un circuit 1 saut vers le pair Python
  (`create`→`created` accepte), envoie un datagramme
  « uTP-compatible » (`DataChecker.could_be_utp`) vers l'echo a
  travers la sortie et verifie la reponse.
- Resultat : clés de session **identiques** des deux cotes (dumps
  `KEYS|` concordants), `decrypt_str` pyipv8 accepte nos cellules
  chiffrees par couches, echo uTP complet.

Deux bugs de fidelite de protocole trouves et corriges grace a ce
montage :

- `cell.rs`/`community.rs` : `send_cell` envoyait le `circuit_id`
  deux fois ; pyipv8 le strippe (`pack_serializable(payload)[4:]`)
  et le reinsere via `unwrap`. Le message cellule est desormais
  `msg_id + payload[4:]`.
- `tribler-crypto/src/ipv8/session.rs` : `generate_session_keys`
  faisait un HKDF extract+expand (sel nul) alors que la reference
  est **EXPAND_ONLY** (`set_hkdf_key(shared_secret)` comme PRK
  directe) → `Hkdf::from_prk`. C'est la raison du "Decryption
  failed" cote Python avant ce fix.

Le jalon Tribler 8.4.3 installe (client complet) reste distinct et
a faire separement.

## Étape 12 (correctif) — garde-fou IPv4 factice, relais UDP first-seen, idempotence `create_e2e` (2026-09-27)

Suite à une revue externe puis vérification directe du code, trois défauts
de sécurité/robustesse ont été corrigés dans `tribler-tunnel` :

- **Fuite réseau via l'IPv4 factice** (`socks5.rs`) : `handle_udp_frame`
  décodait un `circuit_id` depuis l'adresse `CIRCUIT_ID_PORT` sans vérifier
  ni le type ni l'état du circuit visé — un `circuit_id` de circuit `DATA`
  ordinaire pouvait ainsi émettre un vrai datagramme UDP vers l'adresse
  factice. Correctif : `is_ready_rp_circuit` exige un circuit `READY` de
  type `RP_DOWNLOADER`/`RP_SEEDER` ; sinon rejet **sans repli** vers la
  sélection de circuit normale. C'est une **divergence volontaire** par
  rapport à `ipv8-rust-tunnels` (qui vérifie type + clés mais retombe sur
  un circuit `DATA`, laissant la fuite possible) — documentée ici plutôt
  que présentée comme un portage fidèle. Test :
  `socks5_rejects_fake_ip_for_non_rp_circuit`.
- **Relais UDP `last-seen`** (`udp_relay.rs`) : `dial` réécrivait
  `out_client` à chaque datagramme reçu, permettant à un second émetteur
  de détourner le trafic retour. Passage en `first-seen` (le premier
  expéditeur est verrouillé, les suivants sont ignorés), alignant enfin le
  code sur la docstring.
- **Doublon `RP_SEEDER` après retry `create_e2e`** (bug observé en test,
  pas seulement théorique) : chaque retry appelait `create_e2e` avec un
  nouvel `identifier` et un nouveau secret DH, donc une réponse tardive de
  la tentative précédente pouvait lier un second `RP_SEEDER` après le
  succès de la tentative suivante (`left: 2, right: 1` observé). Correctif
  dans `hidden_services.rs` :
  - `Swarm::pending_e2e` (`routing.rs`) retient l'étape de la requête e2e
    en cours par point d'introduction (`Create`/`Building`/`Link`) ;
    `create_e2e` ré-émet le **même** paquet plutôt que d'ouvrir un
    handshake neuf.
  - `Swarm::seen_e2e`/`in_flight_e2e` cachent la réponse `created-e2e` déjà
    produite par (`identifier`, demandeur) et réservent la clé
    **avant** de spawner le traitement (dédup atomique côté seeder).
  - `on_link_e2e` répond `linked-e2e` de façon idempotente si la paire est
    déjà liée, plutôt que de laisser une retransmission expirer.
  - `community.rs::relay_cell` intercepte un `link-e2e` retransmis arrivant
    sur une route de rendez-vous déjà établie et le dispatche localement
    au lieu de le relayer comme une donnée applicative.
  - Test de régression : `hidden_service_e2e_retry_single_rp` (deux
    `create_e2e` en rafale → un seul `RP_SEEDER` lié).
  - **Cause racine du flake observé pendant le développement** :
    `pick_first_hop` (choix du premier saut d'un nouveau circuit) ne
    s'excluait pas du pair `required_exit` — un circuit `RP_DOWNLOADER` à
    2 sauts pouvait tirer le **même** pair comme premier ET dernier saut
    (`EXTEND` vers lui-même), corrompant l'établissement des clés de
    session sur ~30 % des tirages dans une topologie à pairs limités.
    Corrigé en excluant `required_exit` du tirage dans `on_created_e2e`.

Validation : `cargo check/clippy/fmt` propres sur le workspace,
`cargo test --workspace` vert, suite `tribler-tunnel` (12 tests) stable
sur des dizaines d'exécutions séquentielles et parallèles après le
correctif de `pick_first_hop`.

## Étape 12 (correctif) — robustesse des tests e2e sous charge (2026-09-27)

Le handshake e2e (introduction -> peers-request -> create-e2e ->
establish-rendezvous -> created-e2e -> link-e2e -> linked-e2e) echange
~8 datagrammes UDP sans retransmission protocolaire : sous charge
parallele des tests du workspace, une cellule loopback pouvait se
perdre et le test `hidden_service_e2e_roundtrip` expirait a
`e2e_ready` (2/3 echecs isoles observes).

- `tests/circuits_loopback.rs` : `create_e2e_with_retry` — retente le
  `create_e2e` jusqu'a 3 fois (equivalent du `RequestCache` a retry de
  pyipv8), utilise aussi par le test de relais hidden seeding.
- `tribler-bittorrent/tests/anon_download.rs` : meme repli sur la
  creation e2e du telechargement anonyme.

Validation : `verify_all.ps1` vert, le retry a ete observe en action
sous charge (succes a la 2e tentative).

## Étape 12 (partie 5) — `tribler-tunnel` : relais UDP de hidden seeding (2026-09-27)

Le pont entre les circuits e2e et un moteur BitTorrent a socket UDP
concrete (rqbit) est en place :

- `routing.rs` : `circuit_id_to_ip`/`ip_to_circuit_id` — le pair cache
  est adresse `X.X.X.X:1024` ou l'IPv4 encode le `circuit_id`
  (`CIRCUIT_ID_PORT`, comme `packet.rs`/`select_circuit` de
  `ipv8-rust-tunnels`).
- `community.rs` : `subscribe_circuit_data`/`unsubscribe_circuit_data`
  (routage des `CircuitData` par circuit avant le canal general), et
  l'origine des donnees des circuits `RP_*` est reecrite en
  `circuit_id_to_ip(cid):1024` (`data_to_socks5` des tunnels Rust) ;
  `ready_circuits_of_type`.
- `socks5.rs` : une frame UDP vers `IPv4:CIRCUIT_ID_PORT` envoie la
  donnee directement sur le circuit e2e decode de l'adresse.
- `udp_relay.rs` : `dial` (cote downloader : socket loopback ->
  cellules `data` sur le circuit e2e, retour vers le client appris)
  et `serve` (cote seeder : donnees du circuit -> service UDP local,
  reponses -> tunnel). Equivalent du SOCKS5 +
  `set_udp_associate_default_remote` de Tribler sans exiger que le
  moteur parle SOCKS5 UDP (rqbit n'expose qu'une socket concrete).
- Test `hidden_seed_udp_relay_roundtrip` : circuit e2e lie complet,
  faux moteur echo cote seeder, datagramme du client downloader
  revenant en echo a travers le tunnel (10 tests au total).

## Étape 12 (partie 4) — `tribler-tunnel` : CONNECT HTTP par cellules 28/29 (2026-09-27)

Le SOCKS5 CONNECT est desormais fonctionnel (requetes HTTP via le
tunnel — le chemin des annonces de tracker de Tribler) :

- `payload.rs` : `HTTPRequestPayload` (msg 28 : `I, I, address,
  varlenH`) et `HTTPResponsePayload` (msg 29 : `I, I, H, H, varlenH`)
  — numerotation et formats de `tribler/core/tunnel/payload.py`
  (Tribler 8.4.3) et `ipv8-rust-tunnels`.
- `routing.rs` : `PEER_FLAG_EXIT_HTTP = 32768` (extension
  `ipv8-rust-tunnels`), `Circuit.exit_flags` +
  `set_circuit_exit_flags`/`ready_circuits_of_hops_flags`.
- `http_tunnel.rs` : `send_tcp_request` (port de
  `ipv8-rust-tunnels/util.rs` — TCP brut, lecture des en-tetes, corps
  par `Content-Length` ou reassemblage `chunked`), constantes
  `HTTP_RESPONSE_CHUNK=1400`, `MAX_HTTP_REQUESTS_PER_CIRCUIT=5`,
  timeouts 5 s.
- `community.rs` : `perform_http_request` (identifier u32 + cache de
  requetes + recollage des chunks `part`/`total`), `on_http_request`
  cote sortie (flag `EXIT_HTTP` requis, semaphore de 5 requetes par
  circuit, tache dediee), `on_http_response` (acheminement vers le
  cache par identifier).
- `socks5.rs` : `CONNECT` repond `Succeeded` puis relaie la requete
  HTTP brute du client ; `BIND` reste refuse.
- Test `socks5_connect_http_roundtrip` : circuit 1 saut + faux
  tracker HTTP loopback ; annonce bencodee renvoyee par le tunnel et
  verifiee octet par octet (9 tests au total).

## Étape 12 (partie 3) — `tribler-tunnel` : hidden services E2E (2026-09-27)

Le flux de services cachés pyipv8 est implémenté et validé bout en
bout en loopback :

- `hidden_services.rs` : `Swarm` (info-hash -> points d'introduction),
  `join_hidden_swarm`, `send_establish_intro` (circuits `IP_SEEDER`),
  `send_peers_request`/`on_peers_request` (`peers_request` vers un IP
  ou sortie DHT, `peers_response` avec `IntroductionInfo`),
  `send_establish_rendezvous`, `send_create_e2e`/`on_create_e2e`,
  `send_link_e2e`/`on_link_e2e` (identifier partage via le cookie
  rendezvous), `send_linked_e2e`.
- `routing.rs` : types de circuits `IP_SEEDER`/`RP_SEEDER`/
  `RP_DOWNLOADER`, `hs_session_keys` (couche de session E2E
  supplementaire, sens miroir downloader/seeder), `Swarm`.
- `community.rs` : `create_circuit_full` avec `required_exit` (le
  dernier hop — ou le premier si 1 saut — devient point d'intro/
  rendezvous impose), dispatch des messages E2E (9-18) recus en
  cellules `data` ou en paquets tunnel non signes, `send_cell` chiffre
  BACKWARD avec les cles propres de la route quand `rendezvous_relay`,
  `on_create_e2e` repond a `org_address` du payload `data` (pas au
  saut immediat — comportement pyipv8).
- Sortie bidirectionnelle alignee sur pyipv8 : suppression du
  `back_map` — `exit_recv_data` construit la reponse avec
  `org_address` = adresse source reelle du paquet UDP (la sortie
  dediee du repondeur a un port different du destinataire).
- Test `hidden_service_e2e_roundtrip` (8 tests au total dans le
  crate) : le seeder rejoint le swarm, etablit un point
  d'introduction, le downloader decouvre l'IP via `peers_request`,
  cree un circuit E2E vers le rendezvous choisi par le seeder, les
  deux circuits sont lies (`link_e2e`/`linked_e2e`) et les donnees
  circulent dans les deux sens avec la couche `hs_session_keys`.

## Étape 12 (partie 2) — `tribler-tunnel` : sortie bidirectionnelle + SOCKS5 (2026-09-27)

Le tunnel est maintenant bidirectionnel et expose un proxy SOCKS5 :

- `community.rs` : sockets de sortie dédiées — chaque circuit-sortie
  possède sa `UdpSocket` (bind `0.0.0.0:0`) avec tache de reception ;
  `exit.back_map[src] = origine` memorise le demandeur pour router la
  reponse dans le tunnel (`exit_recv_data`, chiffrement BACKWARD).
- `socks5.rs` : proxy SOCKS5 minimal (`ipv8-rust-tunnels/src/socks5.rs`
  en reference) — greeting sans auth, `UDP ASSOCIATE` (CONNECT/BIND →
  `CommandNotSupported`), decapsulage des frames `RSV FRAG ATYP ADDR
  PORT DATA` (IPv4/IPv6/domaine) vers `send_data`, selection sticky
  destination → circuit `READY` du `goal_hops` voulu
  (`ready_circuits_of_hops`), chemin retour : `data_rx` →
  reencapsulation SOCKS5 UDP → `return_map` circuit → (socket, client).
- Test `socks5_udp_associate_roundtrip` : greeting, associate, frame
  UDP vers echo « exterieur », traverse un circuit 1 saut chiffre,
  reponse reencapsulee et verifiee octet par octet.
- Bug corrige en chemin : `handle_associate` relisait le port une
  seconde fois (deja consomme par `read_address`) → blocage.

## Étape 12 (partie 1) — `tribler-tunnel` : circuits fonctionnels en loopback (2026-09-27)

Premier tronçon de la TunnelCommunity, validé par tests loopback réels :

- `cell.rs` : format `CellPayload` fidèle (prefixe 22o + msg 0 +
  circuit_id u32BE + plaintext + relay_early + message), crypto par
  couches (`encrypt_cell`/`decrypt_cell`, compteur explicite 8o +
  tag 16o), `NO_CRYPTO_PACKETS` (create/created), `check_cell_flags`
  et `swap_circuit_id`.
- `payload.rs` : messages tunnel 1-20 (data, create/created,
  extend/extended, ping/pong, destroy, intro/rendezvous, e2e, peers,
  test) au format big-endian pyipv8 (`varlenH`, `[X]`-lists).
- `routing.rs` : `RoutingObject`, `Hop`, `UnverifiedHop` (secret DH
  éphémère conservé jusqu'au `created`), `Circuit` (états
  EXTENDING/READY/CLOSING, `relay_early_count`), `RelayRoute`
  (direction FORWARD/BACKWARD, `rendezvous_relay`).
- `community.rs` : `TunnelCommunity` — `create_circuit`, `send_extend`,
  `join_circuit` (DH `generate_diffie_shared_secret` = DH(tmp2,dh) +
  DH(node_sk,dh), `crypto_auth`, HKDF-SHA256), `relay_created` (routes
  duales + `extended` vers l'amont chiffré BACKWARD), `_ours_on_created_
  extended` (vérif auth + split relais/sorties des candidats),
  `exit_data` (activation au premier octet du bon IP), `send_destroy`
  (paquet signé), `ping`/`pong` de circuit, `send_cell` = `outgoing_
  crypto` (circuit → FORWARD tous hops, exit → BACKWARD, relais →
  direction de l'autre route).
- `tribler-ipv8::endpoint` : `add_raw_prefix_listener` — les cellules
  ne sont pas des `Packet` signés et étaient rejetées par le dispatch.
- Tests `tests/circuits_loopback.rs` : 5 tests sur sockets UDP réels —
  circuit 1 saut et 2 sauts READY (DH complet + relais), données
  traversant 2 hops et sortant en UDP brut au dernier saut, destroy.
- Correction d'un auto-deadlock `std::sync::Mutex` (guard temporaire
  étendu au corps d'un `if let` entourant `relay_cell`).

**Reste pour `[x]`** : SOCKS5, hidden services (`establish_intro`/
`e2e`/rendezvous), socket de sortie UDP dédiée (réponses hors-prefixe),
interops contre un `TunnelCommunity` Python réel.

## Durcissement documentaire du jalon interop (2026-09-27)

Suite à revue externe de la preuve de l'étape 9 :

- `roadmap.md` étape 9 : formulation de la preuve bornée à ce qui a
  été mesuré (discovery signée sur loopback contre pyipv8, 39/39
  paquets de l'essai dans les deux sens, fixtures rejouées en CI) —
  sans extrapoler aux formats non exercés.
- `roadmap.md` étapes 10-11 : les « interop à faire » vagues sont
  remplacés par les scénarios attendus explicites — aller-retour
  `store_value`/`find_values` + jetons avec résultat contrôlé des deux
  côtés (10) ; introductions new-style 233/234 et punctures
  250/232 → 249/231 avec payloads décodés (11).
- `crates/tribler-ipv8/tests/fixtures/README.md` créé : provenance des
  captures (commit pyipv8 `4a294ed1`, commit Tribler `3ac2f4b4`,
  sens de chaque fichier, msg_ids contenus, procédure de
  régénération) — une évolution de la référence ne pourra plus effacer
  la signification du rejeu.
- Distinction des cibles d'interop actée : « venv pyipv8 » (validé)
  ≠ « Tribler 8.4.3 installé » (ressource disponible, non encore
  exercée) ; les résultats futurs seront rapportés séparément.
- `docs/INDEX.md` : scripts d'interop et fixtures référencés.

## Référence supplémentaire : Tribler 8.4.3 installé (2026-09-27)

- `C:\Program Files (x86)\Tribler` documenté dans `AGENTS.md` et
  `docs/INDEX.md` comme référence locale supplémentaire : `Tribler.exe`
  est un noeud Tribler réel (communities IPv8, tunnels, API REST)
  utilisable pour les jalons d'interop ping-pong des étapes `[i]` 10-11
  et des tunnels de l'étape 12 ; `lib/` fournit le pyipv8 figé
  (`.pyc` CPython 3.12 + `ipv8_rust_tunnels.pyd` + `libtorrent`) pour
  un venv interop via `PYTHONPATH` ; `tribler_source/` donne les `.py`
  de la version installée ; `tools/reset*.bat` réinitialise son état.

## Jalon interop — échange enregistré Rust↔pyipv8 (2026-09-27)

L'échange reproductible exigé par la règle de cochage est en place et
**passe** :

- `scripts/interop/py_node.py` : noeud pyipv8 reel (`UDPEndpoint` +
  `DiscoveryCommunity` sur `curve25519`, venv
  `D:\Projet\Tribler_sources\.venv-interop`), journalise chaque
  datagramme en hex et envoie des introduction-request a la cible Rust.
- `scripts/interop/verify_packets.py` : decode chaque paquet
  enregistre (prefix|msg_id|varlenH pubkey|global_time|payload|sig) et
  verifie la signature Ed25519 via le vrai `default_eccrypto` pyipv8.
- `crates/tribler-ipv8/examples/interop_node.rs` : noeud Rust
  (`DiscoveryCommunity`) avec tap de paquets rx/tx (nouveau
  `UdpEndpoint::set_tap`) ecrivant le meme journal hex.
- `scripts/interop_ipv8.ps1` : orchestre les deux noeuds sur loopback,
  verifie 39/39 paquets dans les deux sens, verifie que chaque noeud a
  enregistre l'autre comme pair verifie. Chemins reseables via
  `TRIBLER_PYIPV8` / `TRIBLER_INTEROP_PY`.
- Fixtures enregistrees `crates/tribler-ipv8/tests/fixtures/*.hex`
  (paquets reels pyipv8 + paquets Rust acceptes par pyipv8) rejouees en
  CI par `tests/interop_replay.rs`.
- `roadmap.md` : etape 9 repassee en `[x]` (format filaire + signatures
  + discovery valides contre pyipv8) ; etapes 10-11 restent `[i]` —
  payloads DHT (find/store/tokens) et introductions new-style/punctures
  pas encore exerces contre un noeud Python reel.

## Revue documentaire — cohérence inter-documents (2026-09-27)

Revue critique externe des documents + vérification contre les sources
Python et le code. Corrections appliquées :

- `roadmap.md` : nouveau marqueur `[i]` (implémentée, interop Python en
  attente) appliqué aux étapes 9-11 — la règle de cochage exige la
  validation manuelle, et l'échange reproductible avec un noeud pyipv8
  réel n'a pas encore été fait. Jalon ajouté : script d'interop
  Rust↔pyipv8 avec paquets enregistrés avant de repasser en `[x]`.
- `roadmap.md` étape 12 : le chiffrement de tunnel est
  **ChaCha20-Poly1305** (le texte était resté sur AES-GCM du plan
  initial, contredisant l'étape 2).
- `plan_faisabilite.md` et `architecture.md` : toutes les mentions
  « WebSocket » corrigées en **SSE** — le Python utilise
  `text/event-stream` (`events_endpoint.py`) ; la mention WebSocket
  était une erreur sur la référence elle-même, pas une doc Rust
  obsolète. `AGENTS.md` corrigé de même.
- `decisions/0005-langue-francaise.md` : titre interne corrigé
  « ADR-0003 » → « ADR-0005 » (doublon avec l'ADR licence).
- `plan_faisabilite.md` §4 : la licence renvoyait à ADR-0004 (structure
  workspace) → corrigé vers ADR-0003.
- `correspondance_modules.md` : `api_rest_mapping.md` était annoncé
  « à créer » alors qu'il existe depuis l'étape 6 — corrigé.
- Dates du changelog : les étapes 1-8 étaient datées 2026-09-28 alors
  que git atteste le 2026-09-27 — corrigé.
- `plan_faisabilite.md` : références d'étapes obsolètes corrigées
  (packaging = étapes 17-19, mobile = étape 18).

## Étape 11 — `tribler-ipv8` : framework de communities complet (2026-09-27)

- `peer.rs` reecrit : `Peer` (+ `new_style_intro`, `update_clock`
  Lamport, `last_response`), `WalkableAddress`, `Network` complet —
  `_all_addresses`, `discover_address`, `get_walkable_addresses`
  (filtre service + `old_style`), `get_verified_by_address`,
  `get_introductions_from`, `remove_by_address`, `blacklist`/
  `blacklist_mids`, `reverse_intro` borne FIFO (500).
- `payloads.rs` : `NewIntroductionRequest` (234) et
  `NewIntroductionResponse` (233) au format `ip_address` — bits
  `connection_type(2) + supports_new_style + tunnel + sync + advice`
  cote request, `intro_supports_new_style` en **bit 0** cote response.
- `discovery.rs` reecrit : horloge de Lamport par community
  (`claim_global_time`/`update_global_time`, `% 65536` pour les
  introductions), handlers 233/234/249/250/231/232, reponse new-style
  si le demandeur le supporte, `my_estimated_wan` appris depuis
  `destination_address` hors sous-reseaux LAN (`is_lan_subnet`),
  `get_new_introduction` (pair aleatoire → adresse walkable →
  bootstrap, re-bootstrap 5 %), puncture-request non signe → puncture
  signe vers `wan_walker` (ou `lan_walker` si meme IP WAN), selection
  des introductions LAN/WAN fidele a `introductions` Python.
- Tests `community_framework.rs` (loopback) : introduction nouveau
  style 234→233 avec propagation du flag, adresses walkable apprises
  via introduction puis `get_new_introduction` vers un 3e noeud,
  puncture-request non signe → puncture signe verifie sur socket brut.

## Étape 10 — `tribler-ipv8` : overlay DHT (2026-09-27)

- `dht/routing.rs` : `calc_node_id` (CRC-32 **IEEE/zlib** d'IP masquee
  `0x030f3fff`/`0x0103070f1f3f7fff` + `mid[:17]` — le commentaire Python
  dit "crc32c" mais `binascii.crc32` est CRC-32 IEEE), `distance` XOR
  lexicographique, `Node` (metriques `last_response/last_queries/
  last_ping_sent/failed/rtt`, `blocked` = 10 requetes/5 s, `status`
  BEP-5), `Bucket` (8 noeuds, eviction BAD ou 2x plus lent), `RoutingTable`
  (prefixes binaires, split si le bucket contient `my_node_id`,
  `closest_nodes` par distance+statut).
- `dht/storage.rs` : `Storage` (insertion tete, `id==key` en dernier,
  remplacement si `version>=`, expiration `max_age`).
- `dht/payloads.rs` : msgs 1-10 du protocole DHT + `serialize_value`/
  `unserialize_value` (blobs `0x00+raw` / `0x01+varlenH+I+varlenH+sig`).
- `dht/community.rs` : `DhtCommunity` (`DHTCommunity` +
  `DHTDiscoveryCommunity` fusionnees) : request-cache a oneshot +
  timeouts (5 s / 2 s find), tokens anti-spoofing `sha1("Peer<ip:port,
  b64(mid)>" + secret)` (2 secrets tournants), crawl iteratif
  (`MAX_CRAWL_NODES=8`, `REQUESTS=24`, `TASKS=4`), puncture-request non
  signe + puncture signe, `step()` (PingChurn + ping_all), maintenance
  tokens/noeuds/valeurs.
- `packet.rs` : paquets **non signes** (`lazy_wrapper_unsigned` Python)
  pour msgs 250/232 — `prefix + msg_id + Q + payload` ; `Packet.signed`
  distingue l'absence d'auth.
- `peer.rs` : `remove_peer_key`, `contains_key`.
- Tests loopback : introduction DHT -> ping -> `store_value` signe ->
  `find_values` entre deux noeuds ; rejet de valeur corrompue.

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

## Étape 8 — `tribler-daemon` : executable bout en bout (2026-09-27)

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

## Étape 7 — `tribler-cli` : CLI de pilotage (2026-09-27)

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

## Étape 6 — `tribler-api` : REST + SSE (2026-09-27)

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

## Étape 5 — `tribler-core` : Session + Notifier (2026-09-27)

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

## Étape 4 — `tribler-db` : schema SQLite + migrations (2026-09-27)

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

## Étape 3 — `tribler-bittorrent` : enveloppe `librqbit` (2026-09-27)

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

## Étape 1 — `tribler-format` : bencode, `.torrent`, magnet, `.mdblob` (2026-09-27)

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

## Étape 2 — `tribler-crypto` : hachage et crypto IPv8 (2026-09-27)

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
