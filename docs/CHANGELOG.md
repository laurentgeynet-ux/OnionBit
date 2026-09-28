# Changelog — étapes franchies

Format : une entrée par étape de `docs/plans/roadmap.md`, la plus récente
en haut.

## Correctif : `TunnelCommunityConfig.min_circuits` jamais branché (2026-09-28)

Suite des deux correctifs précédents (timeout de saut + rattrapage
`exit_flags`) : le kill switch se désarme bien et le SOCKS5 accepte la
connexion HTTP, mais les requêtes tracker finissent en `timeout
http-response` en boucle — la seule lane construite ne dispose que
d'**un seul** circuit `READY`, et si son dernier saut annonce
`EXIT_HTTP` sans le servir réellement (pair instable ou mensonger du
réseau public IPv8), il n'existe aucune alternative : le watchdog ne
retente jamais tant qu'un circuit `READY` existe (peu importe s'il
est réellement fonctionnel).

Cause : `TunnelCommunityConfig.min_circuits` (défaut `3`, section
`tunnel_community` du fichier de config) existait dans
`docs/reference_tribler` mais n'était **jamais lu** — `spawn_
circuit_watchdog` (`tribler-core/src/ipv8_stack.rs`) appelait
`build_circuits_if_needed(hops, 1)` en dur, donc une seule lane par
`hops` ne maintenait jamais plus d'un circuit, quelle que soit la
config.

- `Ipv8Config::min_circuits` (nouveau champ, défaut `DEFAULT_MIN_
  CIRCUITS = 3`) : branché depuis `DaemonConfig::tunnel_community.
  min_circuits` (`daemon_config.rs`), stocké sur `Ipv8Stack` et
  transmis à `spawn_circuit_watchdog`, qui l'utilise désormais à la
  place du `1` en dur pour les deux appels à `build_circuits_if_
  needed`. `select_http_circuit` (SOCKS5) tire déjà au hasard parmi
  tous les circuits `READY` portant `EXIT_HTTP` (`usable.shuffle`) :
  avec plusieurs circuits en parallèle, un exit défaillant n'empêche
  plus la lane de fonctionner.

## Correctif : `exit_flags` jamais rattrapé après une introduction tardive (2026-09-28)

Suite du correctif du timeout de saut : une fois celui-ci en place, un
circuit `READY` à 3 sauts se forme bien (kill switch `circuits`
désarmé), mais `aucun circuit HTTP pret` persistait quand même — le
SOCKS5 (`select_http_circuit`) filtre les circuits `READY` par
`exit_flags & PEER_FLAG_EXIT_HTTP`.

Cause : `TunnelCommunity::ours_on_created_extended` fige
`circuit.exit_flags` **une seule fois**, au moment où le dernier saut
répond au `create`/`extend`, en lisant `flag_registry` (rempli par
`register_tunnel_peer` à la réception d'une introduction directe
signée sur le préfixe tunnel). Si ce pair de sortie — appris via la
liste de candidats du saut précédent — n'a pas encore été introduit
directement à cet instant (`flag_registry` ne le connaît pas encore),
`exit_flags` reste `0` **pour toujours** : le circuit est bien
`READY` mais invisible du sélecteur SOCKS5 HTTP.

- `TunnelCommunity::register_tunnel_peer` (`crates/tribler-tunnel/src/
  community.rs`) : en plus d'alimenter `flag_registry`, parcourt
  désormais les circuits dont le dernier saut correspond à ce pair et
  rattrape leur `exit_flags` (+ `notify_circuits_changed`).
- Test de régression `circuit_exit_flags_mis_a_jour_apres_introduction_
  tardive` (`tests/circuits_loopback.rs`) : construit un circuit 1
  saut, vérifie qu'aucun circuit HTTP n'est trouvé avant introduction,
  injecte une `IntroductionResponse` signée après coup via
  `on_raw_datagram`, vérifie que le circuit devient utilisable —
  échoue sans le correctif (`left: [] right: [circuit_id]`).

## Correctif : circuits anonymes bloqués indéfiniment en `EXTENDING` (2026-09-28)

Bug historique : un téléchargement en `hops` 1/2/3 ne démarrait jamais
(fonctionnait en `hops=0`), avec les logs SOCKS5 `aucun circuit HTTP
pret` en boucle et un seul log `tentative de creation proactive de
circuit` sur toute la durée de vie du daemon.

Cause racine : `TunnelCommunity::create_circuit_typed`/`send_extend`
n'avaient aucun mécanisme de timeout (`retry_requests` n'était qu'un
registre passif). Si le premier saut ne répondait jamais à un `create`/
`extend` (paquet UDP perdu, pair injoignable, pair sans le flag
requis…), le circuit restait pour toujours dans l'état `EXTENDING`
(`unverified_hop` non résolu). `build_circuits_if_needed` compte les
circuits `!= READY` comme « en cours » vis-à-vis de `min_circuits`,
donc aucune nouvelle tentative avec un autre pair n'était jamais
lancée — la lane anonyme restait bloquée par le kill switch pour
toujours.

- `TunnelCommunity::spawn_hop_timeout` (`crates/tribler-tunnel/src/
  community.rs`) : après l'envoi d'un `create`/`extend`, programme une
  purge du circuit (`remove_circuit`) si `retry_requests` porte
  toujours le même `identifier` après `CIRCUIT_READY_TIMEOUT_MS`
  (`next_hop_timeout` pyipv8, 10 s) — équivalent du `on_timeout` de
  `NextHopRequestCache` côté Python. Le watchdog de circuits
  (`spawn_circuit_watchdog`, `tribler-core/src/ipv8_stack.rs`) peut
  alors retenter avec un autre pair au tick suivant.

## Suppression des lanceurs `demarrer`/`arreter` (2026-09-28)

Devenus inutiles : `tribler_ui.exe` lance le daemon elle-même
(`daemon_launcher`) et l'arrêt se fait via « Quitter » du systray ou
`PUT /api/shutdown` (qui termine réellement le processus depuis
l'étape 29). `build_dist.ps1` ne les génère plus et nettoie les restes
des builds précédents dans `dist\`.

## UI : lancement automatique du daemon (2026-09-28)

Décision V1 de `flutter_architecture.md` implémentée : plus besoin de
`demarrer.cmd`, lancer `tribler_ui.exe` suffit.

- `core/config/daemon_launcher{,_native,_stub}` : au build de
  `connectionSettingsProvider`, si l'API découverte ne répond pas,
  `tribler-daemon[.exe]` voisin de l'exécutable est lancé détaché
  (`--state-dir <exe>/state` — disposition du bundle `dist\`), puis
  l'API est sondée 30 s (clé et `http_port_running` relus à chaque
  tentative). Daemon déjà vivant → connexion directe ; binaire absent
  (`flutter run`, web) → `null` et repli sur les préférences.
- `TRIBLER_API_KEY` présent = setup externe : jamais de daemon enfant.
  `TRIBLER_DAEMON_EXE` surcharge le chemin du binaire en dev.
- Sonde « API vivante » = toute réponse HTTP, même 401 (même logique
  que `Test-ApiAlive` de `demarrer.ps1`) — testée en loopback
  (`daemon_launcher_test.dart`).
- Symétrie avec l'étape 29 : l'UI lance le daemon, l'item « Ouvrir
  Tribler » du systray lance l'UI.

## Étape 29 — daemon systray Windows + arrêt unifié (2026-09-28)

Le daemon n'affiche plus de fenêtre console : il vit dans la zone de
notification, comme les clients BitTorrent classiques.

- `tribler-daemon` en sous-système GUI (`#![windows_subsystem =
  "windows"]`) ; `--console` rattache la console parente ou en alloue
  une (`AttachConsole`/`AllocConsole` + handles `CONOUT$`, en
  préservant les redirections déjà branchées — `api_parity_run.ps1`
  continue de capter stdout/stderr sans le flag).
- Icône systray via `tray-icon` 0.25.1 (écosystème Tauri) sur un
  thread dédié à pompe Win32 (`GetMessage`/`DispatchMessage` — les
  `WM_COMMAND` du menu sont drainés après chaque dispatch) ; `WM_QUIT`
  termine le thread et retire l'icône. Menu : « Ouvrir Tribler »
  (lance `tribler_ui.exe` voisin de l'exe, désactivé s'il est absent),
  « Démarrer avec Windows » (`HKCU\...\Run\TriblerRustDaemon` via
  `winreg`, item coché reflétant le registre), « Ouvrir le dossier
  des logs », « Quitter ». Icône `tribler.ico` embarquée dans l'exe
  (`embed-resource` + `resources.rc`, ressource 101 — icône de
  fichier comprise), repli `tribler.ico` à côté de l'exe puis RGBA
  généré.
- Arrêt unifié : `ShutdownSignal` (`tokio::sync::Notify`) consommé
  par le graceful shutdown axum ; déclenché par Ctrl-C, « Quitter »
  du tray ou `PUT /api/shutdown` — qui terminait auparavant la
  session sans jamais fermer le processus (le `taskkill /F` de
  `arreter.cmd` faisait le vrai travail). `CoreSession::stop()` est
  désormais idempotent (`stopped: AtomicBool`) : le handler API et la
  séquence principale peuvent l'appeler sans se dédoubler.
- Instance unique par `state_dir` : mutex nommé
  `Local\TriblerRustDaemon-{hash du chemin}` — un second lancement
  sort silencieusement (vérifié : `ExitCode 0`, pas de double icône).
- Réglages : `tray/enabled` dans `configuration.json` (section
  propre au portage), flag `--no-tray` (tests, sessions non
  interactives — `api_parity_run.ps1` et le test e2e le passent).
- `demarrer.ps1` : plus de `-WindowStyle Minimized` (inutile en GUI
  subsystem) ; message d'échec renvoyé vers `state\logs\tribler.log`.
- Vérifié en live : lancement détaché sans console, icône créée,
  API 200, `PUT /api/shutdown` → `signal d'arret recu` →
  `thread systray terminé` → `daemon arrete proprement`, mutex
  d'instance refusant le second processus. Reste la validation
  visuelle du menu (clics, autostart) — étape `[i]`.
- Plan Flutter ajusté : `tray_manager` retiré de
  `flutter_architecture.md` (l'icône appartient au daemon).

## Fix `/api/logging` : journal vide dans l'UI (2026-09-28)

- `tracing_appender::rolling::daily` produit
  `tribler.log.YYYY-MM-DD` — l'extension est la **date**, pas `log`.
  Le handler filtrait `extension == "log"` → aucun candidat →
  l'onglet Journaux affichait « Journal vide » malgré un fichier
  alimenté. Filtre corrigé sur le préfixe `tribler.log`.
- Test `logging_trouve_le_journal_rolle_par_date` fige le nom réel.

## Suivi des pairs et observabilité live (2026-09-28)

- `exit_flags` des circuits désormais rempli depuis le
  `flag_registry` à chaque saut vérifié (était toujours 0 →
  `ready_circuits_of_hops_flags`/`select_circuit` HTTP et l'affichage
  des capacités de sortie ne fonctionnaient pas).
- `GET /api/downloads?get_peers=1` consommé par l'UI : nouvelle
  entité `DownloadPeer` (`ip`, `port`, `extended_version`,
  `direction`, `downrate`/`uprate`, `dtotal`/`utotal`,
  `connection_type`) et liste des pairs connectés dans l'onglet
  « Pairs » du panneau détail.
- Onglets Diagnostic en **polling 2 s** (`tickProvider`) au lieu
  d'instantanés figés : compteurs de circuits/relais/sorties et
  journal se mettent à jour en continu.
- Onglet Journaux : interrupteur **Debug** relié à
  `PUT /api/ipv8/asyncio/debug` — bascule le journal en `debug` à
  chaud pour suivre create/extend/destroy/e2e sans redémarrage.

## Observabilité des circuits anonymes (2026-09-28)

Suivi complet d'un téléchargement anonyme (1-3 sauts) :

- `GET /api/ipv8/tunnel/circuits` aligné sur `circuit_to_dict`
  pyipv8 : ajout de `verified_hops` (mid hex de chaque saut, dans
  l'ordre — la route réellement prise), `unverified_hop`,
  `creation_time` (epoch, `RoutingObject.creation_epoch`), et `state`
  rendu `"CLOSING (raison)"` comme Python.
- Onglet Diagnostic → Circuits : la route s'affiche sous chaque
  circuit (`route : <mid8> → <mid8> → <mid8>…`).
- Rappel d'exploitation : `PUT /api/ipv8/asyncio/debug
  {"enable": true}` bascule le `EnvFilter` à `debug` à chaud — le
  journal `state/logs/tribler.log` (et l'onglet Journaux, via
  `GET /api/logging`) montre alors create/extend/created/destroy,
  e2e et rejets de sortie, sans redémarrage.
- Test `circuit_info_expose_route_et_creation` (loopback, circuit
  réel 2 sauts) fige la shape.

## Conformité des noms API↔UI (2026-09-28)

Audit et alignement des noms divergents entre le contrat Python, les
DTO Rust et les modèles Dart — un seul nom canonique à chaque étage :

- `GET /api/ipv8/tunnel/peers` : shape `{ip, port, mid,
  is_key_compatible, flags[]}` Python (`flags` en liste d'entiers
  `PEER_FLAG_*`, plus `public_key`/bitmask à la frontière API) ;
  `TunnelPeerInfo` Dart mis à jour (modèle + repository + page
  diagnostic).
- Trackers : helper partagé `trackers_json` émettant le
  `TrackerStatusDict` complet (`url`, `peers`, `seeds`, `leeches`,
  `status`, `-1`/`"Not contacted yet"` avant scrape) + pseudo-entrées
  `[DHT]`/`[PeX]`, utilisé par `downloads[].trackers` **et**
  `GET /downloads/{ih}/trackers` ; `DownloadTracker` Dart reflète les
  5 clés (affichage `—` tant que non scrapé).
- `GET /downloads/{ih}/files` : `progress` = fraction 0..1 (était lu
  en octets côté Dart), `priority` retiré du modèle (non émis par
  Python), `included` calculé depuis `selected_files` au lieu de
  `true` en dur.
- Overlays : `overlay_name` + `peers` liste (déjà corrigé dans
  `5b1cd9c`, rappelé ici).
- Test `tracker_lists_follow_trackerstatusdict_shape` fige la shape
  sur les deux endpoints.
- `flutter analyze` propre, 8/8 tests ; `cargo clippy -D warnings` +
  `fmt` propres, 54/54 tests `tribler-api`.

## Étape 20 (avancement) — intégration UI↔daemon validée en live (2026-09-28)

- Inventaire des routes consommées par `app/` : 11 endpoints REST +
  SSE `/api/events` — tous présents dans le routeur et répondent 200
  contre `tribler-daemon` réel (offline) : `downloads`, `settings`,
  `ipv8/overlays`, `tunnel/{circuits,relays,exits,swarms,peers}`,
  `metadata/{torrents/popular,search/local}`, `logging`,
  `events/info`.
- Flux SSE vérifié : `events_start` reçu avec `public_key`/`sessions`/
  `version`.
- Chaîne de connexion conforme : `daemon_api_resolver_native.dart` et
  `dist/demarrer.ps1` lisent `api.key` + `api.http_port_running` de
  `configuration.json` — le daemon les écrit comme attendu (port
  réel publié à chaque démarrage, `http_port=0` restant aléatoire).
- `flutter analyze` propre, 8 tests verts. Reste la validation
  visuelle manuelle du rendu avant de cocher l'étape.

## Étape 12 (clôture) — téléchargement réel via sortie pyipv8 (2026-09-28)

- **`scripts/interop_exit_download.ps1`** +
  `crates/tribler-bittorrent/examples/exit_download_interop.rs` :
  critère final de l'étape 12 validé — rqbit downloader → circuit 2
  sauts → **sortie pyipv8 réelle** (`PEER_FLAG_EXIT_BT`) → socket uTP
  du seeder rqbit, 200 Ko téléchargés et vérifiés octet à octet
  (`INTEROP EXIT DOWNLOAD OK`). `--hops 1` permet le circuit direct.
- **`udp_relay::dial_to`** (`tribler-tunnel`) : variante de `dial` à
  destination filaire explicite (les cellules `data` ciblent la
  socket uTP réelle du seeder ; `dial` conserve l'IPv4 factice des
  circuits e2e liés).
- **`py_tunnel_node.py`** : `--echo-port` optionnel — le noeud peut
  servir de pur relais/exit vers une vraie destination UDP.

## Validation parité réelle — `api_parity_run.ps1` (2026-09-28)

- **`scripts/api_parity_run.ps1`** : orchestrateur du banc — états
  isolés `TSTATEDIR`/`--state-dir`, ports libres dédiés, lancement de
  `Tribler.exe -s` (config `configuration.json` générée avec
  `rss`/`versioning` activés pour que leurs endpoints soient montés)
  et de `tribler-daemon`, attente de disponibilité des deux API,
  exécution d'`api_parity.ps1`, nettoyage (arrêt des processus,
  journaux conservés sous `target/parity/`).
- **Comparaison en sous-ensemble** : le Rust doit fournir toutes les
  clés de premier niveau renvoyées par Python (avec le même type) ; les
  clés supplémentaires sont des extensions documentées (`clierrors`
  dans `/api/downloads`, absent du binaire 8.4.3 mais présent dans les
  sources plus récentes).
- **Résultat mesuré contre le binaire 8.4.3 réel** : 26/27 réponses
  identiques (codes + shapes) ; seule divergence, `/api/rss` GET —
  500 côté Python (méthode inexistante) vs 200 côté Rust, extension
  actée en ADR-0006.

## Étape 28 — `asyncio/*` tokio, items RSS, banc de parité (2026-09-28)

- **`/api/ipv8/asyncio/*`** (`handlers/asyncio.rs` +
  `tribler-core/src/asyncio/`) — adaptation tokio documentée en
  ADR-0006 :
  - `GET/PUT /drift` : `AsyncioMonitor` (`DriftMeasurementStrategy`)
    — tache `interval(walker_interval)` mesurant
    `max(0, reel - attendu)`, historique borne 100 ; 404 `Core drift
    disabled.` ; `PUT {"enable"}` → 400 `incorrect parameters`
    (shape Python sans `success`), `Session not initialized.` sans
    IPv8, corps non-JSON → 500 non geree. `Ipv8Config` porte
    desormais `walker_interval` (propague depuis
    `ipv8/walker_interval`, defaut 0,5 s).
  - `GET /tasks` : `TaskRegistry` (substitut d'`all_tasks()` — tokio
    n'introspecte pas) ; enregistrements aux points de spawn :
    `endpoint`, `bootstrap`, `node_/value_/token_maintenance`
    (`DHTDiscoveryCommunity`), `circuit_watchdog_{hops}` (`Ipv8Stack`),
    `progress` (`CoreSession`), `check` (`WatchFolderService`),
    `check_oldest` (`TorrentChecker`), `check <url>` (`RssWatcher`).
    `running`/`stack` emis `false`/`[]` (pas d'equivalent tokio).
  - `PUT/GET /debug` : `DebugLogBuffer` (deque 50, `%(message)s`) +
    `DebugLogLayer` gatee ; `enable` recharge l'`EnvFilter` global
    (`debug` ↔ directive d'origine) via `reload::Layer` installe dans
    `init_tracing` ; `slow_callback_duration` stocke (defaut 0,1).
    `PUT` sans parametre → 400 `{"success": false}`.
- **`GET /api/rss`** (extension Rust) : migration v5 `rss_items`
  (`feed_url`, `link`, `title`, `infohash`, `first_seen`) ;
  `RssManager` persiste chaque entree `.torrent` vue puis ses
  metadonnees a la resolution ; listing `{items}` tri recent.
- **`scripts/api_parity.ps1`** : banc de parite — meme batterie de
  GET (24 routes) contre `Tribler.exe -s` et le daemon Rust, diff des
  codes HTTP et des shapes JSON (cles:type de premier niveau) ;
  `-FailOnDiff` pour un exit code. Les diffs attendus de `/tasks`
  sont documentes en ADR-0006.
- **ADR-0006** : divergences residuelles actees (exclusion
  `identity/*`, introspection tokio limitee, double format
  speed-test 21/22 u32 + 19/20 u16, extension `GET /api/rss`,
  ecarts de valeurs `eta`/`num_seeds`/priorites, quirks de query
  params conserves).

## Étape 27 — Tunnel avancé : swarm size, peers dht/pex, speed-test SSE (2026-09-28)

- **Speed-test de circuits** (`tribler-tunnel/src/speedtest.rs` +
  dispatch dans `community.rs`) : port de `run_speedtest`
  d'`ipv8-rust-tunnels` — cellules `test-request`(21)/
  `test-response`(22) avec `identifier` **u32** (filaire réel de
  Tribler 8.x) **et** cellules 19/20 `identifier` u16 du backend
  Python pur ; boucle d'envoi throttlée par RTT moyen
  (`target_rtt` 100 ms), snapshots `{tid: [ts_send, bytes_send,
  ts_recv, bytes_recv]}` toutes les 500 ms puis snapshot final
  `done` après drain `2*target_rtt`. `send_cell` renvoie
  désormais le nombre d'octets émis. `create_circuit_with_flags`
  (sélection sortie par flags + premier hop le moins utilisé),
  `await_circuit_ready` (borne `next_hop_timeout` 10 s),
  `remove_circuit` avec `remove_tunnel_delay` 5 s.
- **`GET /api/ipv8/tunnel/circuits/test`** : nouveau circuit
  `SPEED_TEST` + flux `text/event-stream` de lignes
  `speed: {"up","down"}` (MiB/s, calcul `run_speed_test` pyipv8
  reproduit avec `tx_ids`/`rx_ids`) ; `goal_hops` 1..3 sinon 400 ;
  `tunnels` absent → 404 ; échec de création → 500 ; circuit
  détruit après le test. **Quirk conservé** : `request_size`/
  `response_size` en query sont des chaînes → `TypeError` Python →
  500 `{"error":{"handled":false}}` (pas de `validation_middleware`
  dans pyipv8 — le schéma `Integer` est docs-only).
- **`GET /api/ipv8/tunnel/circuits/{cid}/test`** : `circuit_id`
  non numérique → 400 ; tunnel absent ou circuit inconnu → 404 ;
  état ≠ `READY` → 400 ; `DATA` sans `PEER_FLAG_SPEED_TEST` → 400.
- **`GET /api/ipv8/tunnel/swarms/{ih}/size`** :
  `estimate_swarm_size` (crawl `peers-request` : `None`=DHT puis
  IPs découvertes, ≤ `SWARM_SIZE_MAX_REQUESTS` contacts en parallèle,
  comptage des `seeder_pk` uniques `source==PEER_SOURCE_PEX`) ;
  `{"swarms":[]}` sans tunnel ; hex invalide → 500 ; **quirk** :
  `?hops=` arrive en chaîne → `select_circuit` échoue → 0 ;
  infohash ≠ 20o paddé/tronqué comme `struct.pack("20s")`.
- **`GET /api/ipv8/tunnel/peers/dht`** : `DHTIntroPointPayload`
  (`["ip_address","I","varlenH","varlenH"]`) décodé depuis les
  storages DHT locaux (`post_process_values`) → `[{info_hash,
  peers: [{address:{ip,port,public_key}, seeder_pk, source:1}]}]` ;
  `[]` brut sans tunnel/provider ; valeurs malformées ignorées
  (`PackError` → `continue`).
- **`GET /api/ipv8/tunnel/peers/pex`** : nouveau `pex.rs`
  (`PexStore` = `PexCommunity` réduite à ses données : deque bornée
  20 + TTL 300 s + `intro_points_for` de nos annonces) ;
  `start_announce` dans `on_establish_intro`, `stop_announce` +
  déchargement dans `cleanup_exit_socket` (`remove_exit_socket`
  Python — désormais aussi la purge `rendezvous_point_for`/`pex`
  manquante sur destroy d'exit). `on_peers_request` répond d'abord
  depuis le store PEX comme `hidden_services.py`.
- **`DhtCommunity::add_value`** rendu pub (insertion locale directe,
  utilisée par les tests pour peupler `storage.put`).

## Étape 26 — IPv8 réseau, isolation, noblockdht, overlays/statistics (2026-09-28)

- **`StatisticsEndpoint`** (`tribler-ipv8/src/endpoint.rs`) :
  `UdpEndpoint` porte désormais les compteurs Python — agregat
  atomique (`total_up`/`total_down`) + `NetworkStat` par
  `(prefixe community, msg_id)` (`num_up/down`, `bytes_up/down`,
  `first/last_measured_up/down` en `f64` epoch) comptés dans
  `send_to` (tx) et `run` (rx) ; activation par prefixe
  (`enable_community_statistics`) comme `endpoint.enable_community_statistics`.
- **`GET /api/ipv8/network`** : `{b64(mid): {ip, port, public_key
  (b64), services: [b64]}}` sur `Network.all_verified_peers()` ;
  `services_for_peer` ajouté à `Peer`.
- **`GET /api/ipv8/overlays`** réécrit au `OverlaySchema` complet :
  `id`, `overlay_name`, `my_peer` (b64 pub), `global_time` (claim
  lamport), `peers` (`{ip, port, mid b64, public_key b64, flags?}`),
  `statistics` (agregat `NetworkStat`), `max_peers` (30),
  `is_isolated` (reseau propre ≠ `Network` partagé — vrai pour le
  DHT comme en Python), `my_estimated_wan/lan`, `strategies`
  (`RandomWalk`/`RandomChurn`/`PeriodicSimilarity` selon la config
  IPv8 Tribler). Module `overlays.rs` : `OverlayInfo` + decode_map
  par community (`decode_message_name` → `"id:handler"` /
  `"id:unknown"`).
- **`POST /api/ipv8/isolation`** : `ip*`/`port*` + `bootstrapnode`
  ou `exitnode` (400 `missing parameters` sinon ; `exitnode`
  prioritaire si les deux, comme Python). `bootstrapnode` :
  blacklist globale du `Network` partagé + blacklist par overlay +
  `extra_bootstrap` du community + `walk_to` immédiat +
  `DispersyBootstrapper.ip_addresses` ; `exitnode` : `walk_to`
  du tunnel.
- **`GET /api/ipv8/noblockdht/{mid}`** : `connect_peer(mid, peer=
  adresse)` fire-and-forget → `{"success": true}` ; 404
  `{"success":false,"error":"DHT community not found"}` sans
  community, 500 `{"error":{"handled":false}}` sur hex invalide
  (`unhexlify` non géré en Python).
- **`GET /api/ipv8/overlays/statistics`** : `{"statistics":
  [{OverlayClassName: {...}}, {"discovery": agregat}, ...]}` —
  agrégats avec `diff_time = now - first_measured_up`, `{}` sans
  stack. **`POST .../statistics`** : `enable*` requis (400
  `enable flag missing` ), `all` ou `overlay_name` requis (412),
  412 `statistics are not enabled` si l'`UdpEndpoint` n'a pas le
  stockage actif — stats auto-activées au démarrage pour tous les
  overlays comme `session.py` (`enable_statistics`).
- **Alignement tunnel** : `circuits`/`relays`/`exits`/`swarms`/
  `peers` renvoient les collections **vides en 200** quand le
  tunnel n'est pas chargé (`tunnels is None` → `[]` en Python),
  au lieu d'un 400 générique.
- **Communities** : `DiscoveryCommunity` gagne `extra_bootstrap`,
  `add_bootstrapper`, `walk_to`, `overlay_info` ; `DhtCommunity`/
  `ContentDiscoveryCommunity`/`TunnelCommunity` gagnent
  `overlay_info`/`walk_to`/`network`/`claim_global_time` selon
  besoin. Aucune dépendance `tribler-ipv8`→`tribler-tunnel` : la
  decode_map du tunnel vit dans `tribler-tunnel` et est injectée
  dans `OverlayInfo`.
- **Tests** : `endpoint_statistics_par_message` (comptage rx/tx par
  msg_id) + 8 tests REST étape 26 (`network` vide, shapes overlays,
  isolation bootstrapnode/exitnode/400, noblockdht 404/success/500,
  statistics GET/POST 400/412/toggle, collections tunnel vides
  sans stack).

## Étape 25 — `DhtCommunity` dans la stack + `/api/ipv8/dht/*` (2026-09-28)

- **Stack** : `Ipv8Stack.dht: Option<Arc<DhtCommunity>>` créé si
  `ipv8.enabled` + `dht_discovery/enabled` (precondition du
  `DHTDiscoveryComponent` Python — notre `DhtCommunity` couvre
  `DHTCommunity` + `DHTDiscoveryCommunity`). `CoreSession::dht()`
  = `session.get_overlay(DHTCommunity)`.
- **Maintenance** : tâche Tokio aux cadences Python — `step`
  (`PingChurn.take_step` + `ping_all`) à 0,5 s, `node_maintenance`
  60 s, `value_maintenance` 3600 s, `token_maintenance` 300 s +
  appel immédiat au démarrage ; arrêt propre via `Ipv8Stack::stop`.
  `my_estimated_wan/lan` propagés depuis la discovery à chaque tick
  (même `my_peer` partagé en Python) — `DhtCommunity.my_wan`/
  `my_lan` sont devenus mutables (`set_my_wan`/`set_my_lan`).
- **Bootstrap** : `walk_to` vers chaque noeud d'amorcage résolu
  (intro-requests sous préfixe DHT) avant la marche discovery.
- **API** (`dht_endpoint.py` pyipv8, 7 routes) : `statistics`
  (`peer_id`, `num_tokens`, `endpoints[]` par classe d'adresse,
  `num_peers_in_store`/`num_store_for_me`), `values` (objet indexé
  par clé hex), `values/{key}` GET (lookup + `debug` : `requests`,
  `responses`, `responses_with_nodes`, `responses_with_values`,
  `time`) et PUT (`{"value": "<hex>"}`, `sign=True`,
  `incorrect parameters` 400), `peers/{mid}` (`connect_peer`),
  `buckets` + `buckets/{prefix}/refresh`.
- **Fidélité des erreurs** : 404 `{"success":false,"error":"DHT
  community not found"}` sauf `buckets` (200 `[]`) et `refresh`
  (400 `DHT community is not loaded`) ; `unhexlify`/`DHTError` →
  500 `{"error":{"handled":false}}` (`error_middleware`) ; `refresh`
  d'un prefixe inconnu → 400 `no such bucket`, d'une `DHTError` →
  200 `{"success":false,"error":e}`. `find` propage l'erreur de
  construction du `Crawl` (table vide) comme Python — levée aussi
  en mode valeurs. Divergence documentée : `distance` en décimale
  chaîne (int Python 160 bits > u128 JSON).
- **Accesseurs** `tribler-ipv8` : `stats_snapshot`/`buckets_snapshot`/
  `stored_values`/`find_values_debug`/`refresh_bucket`,
  `Node::failed`, `Storage::items_snapshot` — instantanés read-only,
  aucun mutex interne exposé.
- **Tests** : `dht_routes_sans_community` (formes d'erreur par
  route) + `dht_routes_avec_community` (stack IPv8 loopback sans
  bootstrap, 7 routes exercées).

## Étape 24 — Topics SSE complets + vraie `public_key` (2026-09-28)

- **`public_key` réelle** : `events_start` et `/api/events/info`
  rapportent `Ipv8Stack::public_key_hex()` via `CoreSession` (`""` si
  IPv8 désactivé) — fin du placeholder.
- **Nouveaux topics** (format `Notification` Python, `data:` JSON) :
  `remote_query_results`, `local_query_results`, `tunnel_removed`,
  `tribler_shutdown_state`, `low_space`, `tribler_exception`,
  `report_config_error`, `ask_add_download`, `tribler_new_version`
  (variante prête, pas d'émetteur — mort en Python 8.x aussi).
- **`remote_query_results`** : `pending_selects` de
  `ContentDiscoveryCommunity` porte désormais un `SelectRequest`
  complet (adresse du pair, `packets_limit` = 10, `peer_responded`,
  `processing_callback`) — un pair muet à l'expiration est retiré du
  réseau comme `_on_query_timeout`. `process_select_response`
  retourne les `to_simple_dict()` des objets `NEW` (dedup
  `(public_key, id_)` = `DUPLICATE_OBJECT` exclu, comme `notify_gui`).
- **`tunnel_removed`** : `TunnelCommunity` émet `circuit_removed`
  (`Circuit`/`RelayRoute`/`TunnelExitSocket` + stats + uptime +
  raison) sur un canal broadcast ; `Ipv8Stack` le relaie au
  `Notifier` sans dépendance tunnel→core.
- **`tribler_shutdown_state`** : `stop()` émet les phases Python
  (checker → overlays IPv8 → download manager → SOCKS5 → base → GUI
  « Going dark. »).
- **`low_space`** : sonde `fs2` à l'ajout (`downloads_dir`, seuil
  1 Gio) — le topic est mort en Python 8.x ; extension documentée.
- **`report_config_error`** : `DaemonConfig::load_report` remonte
  l'erreur de parse ; le daemon la notifie après démarrage.
- **`ask_add_download`** : `PUT /api/downloads` avec `cli` +
  `libtorrent/ask_download_settings` → `{"started": false}` + notif.
- **`local_query_results`** : émis par `GET
  /api/metadata/search/local`.
- **Correctif de fidélité** : `DownloadStateChanged` →
  `torrent_status_changed` (nom `DownloadStatus` Python) ;
  `download_state_changed` reste une extension de progression.
- **Tests** : sérialisation SSE des 9 nouveaux topics,
  `torrent_status_changed`, `local_query_results`,
  `ask_add_download`, callback `remote_select` (ipv8).

## Étape 23 — Trackers par download (2026-09-28)

- **Routes** : `PUT /api/downloads/{ih}/trackers` (`{"added": true}`),
  `DELETE …/trackers` (`{"removed": true}`), `PUT …/default_trackers`
  (`{"added": true}`), `PUT …/tracker_force_announce`
  (`{"forced": true}` — rendu même pour une URL inconnue, quirk Python).
  404 avant validation du corps ; `url` absent → 400
  `"url parameter missing"` ; erreurs moteur → 500 `handled`.
- **`removed_trackers` (migration v4)** : comme `tdef.atp.trackers`
  Python, le retrait survit aux re-adds — la source est réécrite sans
  ses trackers (`tribler_format::torrent::strip_trackers` chirurgical,
  infohash préservé ; `magnet::strip_trackers` retire les `tr`) et
  l'ensemble effectif `(source ∪ extra) ∖ removed` est passé à
  `opts.trackers` (librqbit fusionne toujours source + options).
- **Trackers par défaut** : `download_defaults/trackers_file` lu au
  format uTorrent (lignes non vides) et synchronisé depuis
  `trackers_file_sync_url` avec le TTL Python d'une heure
  (`sync_default_trackers_file`) — URL validée par la politique
  anti-SSRF. Appliqués à chaque ajout (sauf torrent `private`) et à la
  demande via la route, persistés dans `extra_trackers`.
- **Divergences rqbit consignées** : pas d'annonce à chaud pour un
  tracker ajouté/retiré (effectif au re-add) ; `force_announce`
  réannonce tous les trackers (pause+unpause).
- **Correctif** : `parse_id_or_hash` lisait un infohash tout-chiffres
  (`"00..0"`) comme un id interne — le lookup API est désormais strict
  hex (`find_download_hex`/`get_by_hash`, hex-40 prioritaire).

## Étape 22 — Réglages par download persistés + PATCH complet (2026-09-28)

- **Migration `tribler-db` v3** : colonnes `downloads` —
  `safe_seeding`, `upload_limit`, `download_limit`, `seeding_ratio`,
  `auto_managed`, `queue_position`, `completed_dir`, `selected_files`,
  `file_priorities`, `extra_trackers`, `time_finished`. Upsert complet,
  restauration des réglages aux re-add internes (équivalent des
  `dlcheckpoints`/`DownloadConfig` Python). Tests de migration
  v1→v3, réouverture idempotente, rejet de schéma futur, round-trip.
- **`tribler-bittorrent`** : `AddDownloadOptions` (paused, fichiers
  sélectionnés, dossier, trackers, limites par torrent via
  `AddTorrentOptions::ratelimits`) ; `update_only_files`, limites de
  session à chaud, bitfield des pièces en base64 (`api_dump_haves`),
  stats par pair, `force_announce`, `total_pieces`, fichiers/traqueurs
  exposés par `Download`.
- **`tribler-core`** : `DownloadDefaults` dans `CoreConfig` (alimenté
  par `download_defaults` de `configuration.json`), politique d'arrêt de
  seed (`seeding_ratio`/`seeding_mode`/`seeding_time`, `safe_seeding`)
  dans la boucle de progression, `pause`/`resume` idempotents (parité
  Python), `remove_engine_only` pour les re-add internes, opérations
  `recheck`/`move_storage`/`set_selected_files`/file_priority/queue/
  `auto_managed` persistées.
- **`PATCH /api/downloads/{infohash}`** : sémantique Python complète —
  404 avant validation, exclusivité `anon_hops`, `resume`/`stop`
  (`user_stopped` persisté), `recheck` et `move_storage` par
  remove+re-add (revalidation naturelle, `fastresume` inerte sans
  persistence rqbit), `dest_dir` manquant → 500 `KeyError` handled,
  dossier absent → 400, déplacement no-op → `modified:false`,
  `selected_files` bornés, `file_priority` 0..=7, `queue_position` ∈
  {up,top,down,bottom} (ordre logique persisté — pas de file rqbit),
  `auto_managed` booléen, limites/`seeding_ratio` persistés et réappliqués
  au re-add (rqbit ne mute pas les ratelimits à chaud par torrent).
- **`GET /api/downloads`** : flags `get_peers` (sous-ensemble rqbit),
  `get_pieces` (base64 MSB-first), `get_availability` (approximation
  `float(num_seeds)` — librqbit ne fusionne pas les bitfields pairs) ;
  actifs seulement quand le paramètre vaut `"1"`, comme Python. DTO
  rempli depuis la ligne persistée (destination, limites, trackers
  union announce/announce-list + ajouts, `total_pieces`, `time_finished`).
- Divergences rqbit consignées dans `api_rest_mapping.md` : ratelimits
  appliquées au re-add, `file_priority`/`queue_position`/`auto_managed`
  persistés sans ordonnancement moteur, `availability` approximée.

## Correctif — Connexion UI après activation de la clé API (2026-09-28)

- **Auto-découverte de la session daemon** (`app/lib/core/config/daemon_api_resolver*.dart`) :
  même mécanisme que `session_resolver` d'eMule-Rust — l'UI lit
  `configuration.json` du daemon (`api.key` + `api.http_port_running`)
  dans les répertoires candidats (`<exe>/state`, `<cwd>/.tribler`, …)
  ou `TRIBLER_API_KEY`/`TRIBLER_API` en environnement, puis retombe sur
  les préférences. Un réglage utilisateur vers un daemon distant
  (non-loopback) reste prioritaire. Conditional import : stub `null`
  sur web.
- **Lanceurs `dist/`** (`scripts/build_dist.ps1`) : refondus en
  `demarrer.ps1`/`arreter.ps1` + wrappers `.cmd` minimalistes — la
  syntaxe `for /f` + quoting emboîté de cmd ne supporte pas la lecture
  de `configuration.json`. Le port réel est relu à chaque sonde,
  toute réponse HTTP (dont 401) signifie « API en vie », et
  `PUT /api/shutdown` envoie `X-Api-Key`.

## Étape 21 — Configuration persistée et clé API (2026-09-28)

- **`DaemonConfig` persistée** (`tribler-core/daemon_config.rs`) :
  arbre `TriblerConfig` serde (`api`, `ipv8`, `libtorrent` +
  `download_defaults`, `tunnel_community`, `rss`, `watch_folder`,
  `torrent_checker`, `dht_discovery`, `versioning`, `statistics`,
  `state_dir`) lu/écrit dans `state_dir/configuration.json`.
  Clés inconnues conservées au merge (compat ascendante). `api/key`
  générée hex (32 car.) au premier démarrage ; `api/http_port_running`
  réécrit après le bind réel (port `0` = aléatoire, comme Python).
- **Authentification par clé API** (`tribler-api/auth.rs`) : middleware
  axum strictement équivalent à `ApiKeyMiddleware` Python — clé lue dans
  `X-Api-Key`, puis `?key=`, puis le cookie `api_key`, même en loopback ;
  rejet `401 {"error": {"handled": true, "message": "Unauthorized
  access"}}`. `DefaultBodyLimit` aligné à 16 Mio (`MAX_REQUEST_SIZE`).
- **`/api/settings` adossé au fichier** (`tribler-api/handlers/settings.rs`) :
  `GET` rend l'arbre persisté complet ; `POST` merge récursivement le JSON,
  réécrit `configuration.json` (équivalent `config.write()`) et applique à
  chaud les clés connues (`rss`, `watch_folder`, `libtorrent`,
  `tunnel_community`, `api`, `ipv8`) via `apply_service_settings`.
- **`tribler-daemon`** : charge `configuration.json`, applique les
  overrides CLI (`--api`, `--ipv8-port`, `--bootstrap`, …), publie le
  port réel dans `api/http_port_running` et la clé dans l'`AppState`.
- **`tribler-cli`** : options `--api-key` et `--state-dir` ; découverte
  automatique de la clé et du port dans `state_dir/configuration.json`
  (défaut `.tribler`) quand rien n'est passé explicitement.
- **Tests** : auth 401/en-tête/query/cookie, round-trip de settings
  persistés (`tribler-api/tests/api.rs`), e2e daemon avec clé générée +
  `http_port_running` (`tribler-daemon/tests/daemon.rs`), découverte de
  clé CLI (`tribler-cli/tests/cli.rs`).

## Correctif — Parité `/api/libtorrent` (`hop`), création proactive de circuits & garde d'ajout (2026-09-28)

- **Parité protocolaire `/api/libtorrent`** :
  - `tribler-api/handlers/libtorrent.rs` : support du paramètre de requête `?hop={0..3}` pour `/api/libtorrent/settings` et `/api/libtorrent/session` en stricte conformité avec Python Tribler (`libtorrent_endpoint.py`), tout en conservant `session` en alias pour la rétro-compatibilité.
  - Mise à jour de `docs/reference_tribler/api_endpoints_complet.md` (section 5 validée ✅).
  - Tests automatisés dans `tribler-api/tests/api.rs`.
- **Ajout de téléchargements anonymes (1, 2, 3 sauts)** :
  - `tribler-network-policy/kill_switch.rs` : ajout de `guard_add(&self)` qui n'interdit l'ajout qu'en cas de panne critique du proxy SOCKS5 local ou d'arrêt d'urgence manuel, sans bloquer l'ingestion tant que les circuits overlay sont en cours d'établissement.
  - `tribler-bittorrent/engine.rs` : utilisation de `guard_add` lors de l'ajout d'un torrent. Le flux réseau SOCKS5 rejette silencieusement les paquets UDP tant qu'aucun circuit n'est prêt (zéro fuite IP garantie), permettant au téléchargement de patienter et de démarrer dès que le circuit est fonctionnel.
- **Construction proactive des circuits dans `TunnelCommunity`** :
  - `tribler-ipv8/peer.rs` : exposition de `all_verified_peers()` dans `Network`.
  - `tribler-tunnel/community.rs` : implémentation de `build_circuits_if_needed(hops, min_circuits)` qui choisit des pairs vérifiés (priorité aux drapeaux de sortie BitTorrent `PEER_FLAG_EXIT_BT` à 1 saut, relais `PEER_FLAG_RELAY` à 2+ sauts, ou pairs vérifiés) et déclenche `create_circuit`.
  - `tribler-core/ipv8_stack.rs` : appel proactif de `build_circuits_if_needed` dès le démarrage des lanes anonymes et à chaque cycle (5s) du watchdog.
- **Packaging & Déploiement** :
  - Binaires release `tribler-daemon.exe` et `tribler-cli.exe` recompilés et déployés dans `dist/` et `C:\Users\Lou\Desktop\Tribler-Rust-Torrent`.

## Correctif — Initialisation des lanes anonymes (sauts 1, 2, 3) & persistance DHT (2026-09-28)

- **Correction du conflit DHT sur les lanes anonymes** :
  - `tribler-bittorrent/config.rs` : configuration de `librqbit::DhtSessionConfig` avec `persistence: None` pour la DHT en mémoire éphémère (évite le verrouillage concurrent de `dht.json` et les collisions de port persistant sur Windows `WSAEADDRINUSE 10048`).
  - `tribler-core/ipv8_stack.rs` : isolation stricte des moteurs BitTorrent anonymes créés par `anon_engine(hops)` : désactivation explicite de la DHT mainline (`enable_dht = false`), de la découverte locale (`disable_lsd = true`) et du port d'écoute direct (`listen_port = None`). Un téléchargement anonyme ne doit jamais émettre de paquets UDP DHT/LSD hors du circuit SOCKS5.
- **Précision des messages d'erreur API** :
  - `tribler-api/handlers/downloads.rs` : distinction des erreurs réelles de parsing de fichier (`CoreError::Format(_) -> "corrupt torrent file"`) par rapport aux erreurs d'état du moteur ou de réseau, évitant de masquer les erreurs d'infrastructure sous un faux message de fichier corrompu.
- **Tests & Packaging** :
  - Ajout d'un test de non-régression dans `crates/tribler-core/tests/circuit_death.rs` (`anon_engine_demarre_proprement_avec_dht_active_sur_session`) validant que les 3 lanes anonymes s'initialisent correctement même quand la DHT est active sur la session principale.
  - Reconstruction release complète via `scripts/build_dist.ps1 -SkipCheck` et synchronisation vers `C:\Users\Lou\Desktop\Tribler-Rust-Torrent`.

## Étape 20 (partie 2) — Câblage complet de l'interface Flutter et du backend (2026-09-28)

- **Câblage des Trackers et Swarm** :
  - `GET /api/downloads/{ih}/trackers` et `PUT /api/downloads/{ih}/trackers` exposés dans `DownloadsRepository`, `RestDownloadsRepository` et `downloadTrackersProvider`.
  - Panneau de détail : onglet « Trackers » affichant les trackers réels avec leur statut et le nombre de pairs découverts, et dialogue d'ajout de tracker en direct.
  - Onglet « Pairs » affichant les statistiques de l'essaim (seeders, leechers, total pairs, débits instantanés, mode réseau direct/tunnel).
- **Actions sur les Téléchargements & Menu contextuel** :
  - Menu contextuel complet (clic droit sur bureau ou appui long sur compact) : Reprendre / Pause, Ouvrir le dossier dans l'explorateur natif, Sélecteur de niveau d'anonymat (0 à 3 sauts), Copier le lien magnet, Copier l'info-hash, Supprimer.
  - Onglet « Détails » enrichi de boutons d'action rapide (Ouvrir le dossier, Copier le lien magnet, Copier l'info-hash).
  - Onglet « Fichiers » avec bouton d'ouverture directe de l'emplacement de chaque fichier individuel (`openPath`).
  - Dialogue d'ajout : sélecteur de dossier natif (`getDirectoryPath`) et transmission effective du dossier de destination personnalisé en mode magnet comme en upload `.torrent` brut.
- **Indexation locale & Recherche / Découverte** :
  - `tribler-core` : indexation automatique dans `channel_node` (`metadata_type = 300`) des téléchargements ajoutés et restaurés.
  - `tribler-api` : enrichissement dynamique des endpoints `/api/metadata/torrents/popular` et `/api/metadata/torrents/local_search` pour inclure immédiatement les torrents actifs de la session.
- **Réglages & Préférences** :
  - Section « Téléchargements par défaut » dans la page Réglages avec sélection et enregistrement du répertoire par défaut via `POST /api/settings`.
  - Backend : prise en compte à chaud de `download_defaults.saveas` dans `ServiceOverrides`, `effective_config` et application immédiate.
- **Journaux et Diagnostique** :
  - Configuration de `tracing_appender` dans `tribler-daemon` écrivant dans `<state_dir>/logs/tribler.log` (rotation quotidienne) en plus de stdout, rendant l'onglet « Journaux » fonctionnel.
- **Layout & Polissage UI** :
  - Correction de l'espacement et des débordements de texte dans la barre d'état et le badge de statut des téléchargements.
  - Packaging complet de `dist\` et synchronisation avec le dossier de test Bureau.

## Étape 20 (partie 1) — coquille Flutter + packaging `dist/` (2026-09-28)

- `scripts/build_dist.ps1` : assemble un dossier **portable** `dist\`
  (daemon + CLI release, UI Flutter Windows release, `demarrer.cmd`,
  `arreter.cmd`, `build-manifest.json`). Le lanceur démarre le daemon
  (console minimisée, `--state-dir %~dp0state`), attend l'API
  `127.0.0.1:8085` (30 s max), puis ouvre `tribler_ui.exe`.
  `arreter.cmd` fait `PUT /api/shutdown` puis `taskkill` en filet.
  `dist\state\` (données utilisateur) n'est jamais effacé par le build.
  Vérifié de bout en bout : daemon démarré, API `/api/events/info` OK,
  UI connectée (2 sessions TCP REST+SSE).
- **Activation IPv8 et bootstrap réel dans `tribler-daemon`** :
  `Ipv8Config::production()` avec les 20 nœuds officiels `DISPERSY_BOOTSTRAPPER`
  (TU Delft / Tribler), résolution DNS asynchrone des adresses `dispersy*.tribler.org`,
  repli automatique sur port éphémère si le port UDP 8090 est occupé,
  options CLI `--no-ipv8`, `--no-anonymity`, `--ipv8-port`, `--bootstrap`.
  Binaires release régénérés dans `dist\` et le dossier de bureau.
- **Support de l'upload binaire brut de `.torrent` dans `tribler-api`** :
  `PUT /api/downloads` accepte désormais à la fois le JSON standard et
  le flux binaire brut (`Content-Type: applications/x-bittorrent` ou
  `application/x-bittorrent`) avec query parameters (`anon_hops`, `safe_seeding`,
  `paused`), en conformité exacte avec le contrat de Tribler Python.
  Augmentation du `DefaultBodyLimit` à 20 Mo pour accepter les gros fichiers
  `.torrent`. Résout l'erreur `10053` (`WSAECONNABORTED`) lors de l'ajout depuis l'UI.

Première implémentation de l'UI dans `app/` (le brouillon initial est
remplacé) :

- **Thème repris de l'app eMule de référence** : Material 3, seed
  `0xFF2F6FED`, palette d'accents identique, mode clair/sombre/auto
  (`ThemeMode.system` par défaut), persisté via `shared_preferences`,
  tokens `AppSpacing`/`AppRadii`.
- **Shell responsive** : sidebar fixe ~216 px type Tribler (bouton
  « Ajouter », sous-filtres Téléchargements avec compteurs,
  Rechercher, Réglages, Diagnostic) au-delà de 600 dp, `NavigationBar`
  compacte en dessous ; barre de recherche globale (debounce 300 ms,
  navigue vers `/search`) ; barre d'état (connexion daemon SSE,
  état honnête de la lane anonyme via les circuits `READY`, débits
  globaux).
- **Downloads** : poll 2 s + invalidation SSE (`download_state_changed`,
  `torrent_finished`), table desktop (Nom/Taille/Progression/État/
  ↓/↑/ETA/Pairs/Anonymat) + liste compacte avec bottom sheet,
  panneau de détail à onglets (Détails/Fichiers/Trackers/Pairs),
  multi-sélection avec barre d'actions pause/reprendre/supprimer,
  dialogue d'ajout (magnet/URI ou `.torrent`, aperçu `torrentinfo`,
  destination, Direct/Anonyme 1-3 sauts, `safe_seeding` auto).
- **Rechercher** : torrents populaires en contenu initial, résultats
  locaux immédiats à la frappe ; recherche distante lancée en
  parallèle — **écart constaté** : le backend intègre les réponses
  dans `channel_node` sans pousser `remote_query_results`, l'UI
  re-sonde donc `search/local` pendant ~10 s et marque « réseau »
  les nouvelles entrées.
- **Réglages** : apparence (accent + mode), connexion daemon
  (URL/clé persistées), état du daemon + arrêt (`/api/shutdown`).
- **Diagnostic** : onglets overlays/circuits/relais/sorties/swarms/
  pairs/journaux (`/api/ipv8/*`, `/api/logging` en texte brut).
- **Web-safe** : `window_manager` isolé derrière un import
  conditionnel (`desktop_shell.dart`/`_native`/`_stub`) ; `web/`
  généré, `flutter build web` et `flutter build windows` OK.
- Validation : `dart format`, `flutter analyze` propre, 8 tests
  (formateurs, parseur SSE, smoke test du shell avec providers
  surchargés). La validation visuelle contre un daemon réel reste à
  faire → l'étape 20 est marquée `[i]` dans la roadmap.

## Changement de plan : mobile = pilotage distant, desktop d'abord (2026-09-28)

Décision utilisateur : Android/iOS seront une **interface de pilotage
à distance** (REST+SSE vers un daemon desktop), pas un portage du
daemon — l'étape 19 (builds mobiles) est remplacée, la façade FFI
`tribler-mobile` devient inutile (le document de surface reste en
référence). La prochaine phase est l'interface **Windows desktop**.

- `docs/plans/flutter_architecture.md` : plan d'architecture de
  l'étape 20 — stack Flutter 3.47/Riverpod 3/go_router, arborescence
  `core/`+`features/` en `data/domain/presentation` (pattern
  `C:\Emule-Sion-UI-UX\app`), `ApiClient` REST+SSE maison (le
  `RpcClient` WebSocket de la référence devient un client
  `text/event-stream`), correspondance features↔endpoints, cycle de
  vie window/tray, stratégie de tests, sous-étapes.
- `CoreSession::pause_all`/`resume_all` restent (utiles au daemon
  desktop : arrêt rapide).

## Préparation étape 19 — façade FFI mobile (2026-09-28)

L'étape 19 (builds mobiles) n'est **pas** démarrée — ce jalon fige
son contrat d'entrée, conformément à l'ordre d'audit :

- `docs/plans/mobile_ffi_surface.md` : surface FFI minimale —
  fonctions plates à payloads JSON (`tribler_start`/`stop`,
  `pause_all`/`resume_all`, `add_download`, `list_downloads`,
  `download_action`, `set_event_callback`, `free_string`), règles de
  propriété mémoire et de threading du callback notifications
  (topics identiques aux trames SSE), règles de cycle de vie
  (suspension → `pause_all` ; kill → `stop` borné ~2 s), codes
  d'erreur, choix de binding (UniFFI/JNI Android, cbindgen iOS).
- `CoreSession::pause_all`/`resume_all` + `BtEngine::pause_all`/
  `resume_all` : implémentés — itèrent tous les moteurs (principal +
  lanes anonymes), erreurs unitaires collectées non fatales, kill
  switch respecté par lane. Test
  `lifecycle::pause_all_resume_all_basculent_tous_les_telechargements`.
- `AppState::new(session)` factorisé (constructeur unique).

## Arbitrage étape 12 + contrat API downloads/events (2026-09-27)

- **Étape 12 repasse `[i]`** : les preuves existantes couvrent
  l'interopérabilité *protocolaire* (pyipv8 + Tribler 8.4.3 relais),
  mais pas littéralement « téléchargement via le réseau Tribler
  existant » — critère conservé ouvert ; le bloquant documenté est
  l'absence de sortie Tribler (`exitnode_enabled` non exposable).
  Banc de clôture retenu : rqbit → circuit → sortie pyipv8
  (`EXIT_BT`) → seeder.
- **`eta`** : chaîne formatée → **float de secondes**, formule
  `get_eta()` Python `(1-progress)*size/max(download_rate,1e-6)`.
- **`num_seeds`/`num_peers`** : remplis depuis le scrape
  `torrent_state` du torrent checker (`max(scraped, connectés)`),
  au lieu de `0` constant. `num_connected_seeds` reste 0 —
  librqbit ne distingue pas seeds/leechers connectés (divergence
  documentée).
- **`PUT /api/statistics/dirspace`** : route Python exacte
  (corps `{"directory"}`, réponse `{"statistics": {…}}`, remontée
  au premier ancêtre existant, 404 sinon) ; le `GET ?path=` reste
  en confort avec la même forme de réponse.
- **`GET /api/events/info`** : implémenté (`{"public_key",
  "version", "sessions"}`) avec compteur réel de flux SSE ouverts
  (+1/−1 à la connexion/déconnexion), partagé avec le message
  `events_start`.
- **`GET /api/downloads/clierrors`** : implémenté — file
  `unhandled_cli_log` fidèle (insertions en tête, borne 100,
  drainée par le GET), alimentée par les erreurs de
  `PUT /api/downloads` quand `cli:true` ; champ `clierrors` de
  `GET /api/downloads` = longueur de la file.
- `hops`/`anon_download` : déjà remplis (étape 15) — désormais
  couverts par un test de contrat.
- Tests : `events_info_et_dirspace_contrat_python`,
  `clierrors_journalise_puis_vide`, `downloads_eta_est_un_nombre`.

## Durcissement des bancs d'interop (2026-09-27)

Les bancs produisent des journaux partagés : on retire ce qui n'a pas
à y figurer et on rend le vérificateur exigeant sur ce qu'il accepte.

- `scripts/interop/verify_packets.py` : `--allow-msg-id` (whitelist
  CSV) — tout `msg_id` hors liste est un échec, même si la signature
  est valide ; compteurs séparés par `msg_id` et par type
  (signé/non-signé/invalide) au lieu d'un total unique ; layout
  `dist` décodé uniquement pour les messages qui le portent
  (246/245/234/233/249/231), fidèle au fix filaire de l'étape 10.
  Premier run : la whitelist a immédiatement détecté un
  `similarity-request` pyipv8 (msg_id=1, famille DiscoveryCommunity,
  payload.py) que l'ancien total absorbait — whitelist
  `interop_ipv8.ps1` = `1,2,3,4,246,245,250,249`.
- Secrets de session : dump `KEYS|` (clés forward/backward + sels)
  supprimé de `py_tunnel_node.py` et des exemples
  `tribler_relay_interop`/`tunnel_interop_node` ; accesseur
  `TunnelCommunity::debug_session_keys` supprimé (n'existait que pour
  ces dumps). Les `KEY|` (clé publique, non secrète mais redondante
  — la coordination passe par les fichiers `--key-file`) sont
  retirés de `py_dht_node.py`, `dht_interop_node` et
  `discovery_interop_node`.
- `scripts/interop_tribler.ps1` : ports fixes `22090`/`23100`/`22091`
  → tirage de ports libres (UDP×2 distincts + TCP) propagés à la
  config Tribler, l'API, l'echo et les arguments Rust — plus de
  collision avec un Tribler local ou un reste de run.
- Validé : `interop_ipv8.ps1`, `interop_dht.ps1`,
  `interop_discovery.ps1`, `interop_tunnel.ps1` et
  `interop_tribler.ps1` tous verts après durcissement ; aucun `KEYS|`
  ou `KEY|` dans les journaux produits.

## Durcissement étapes 13/16 — rupture de circuit, proxy vivant (2026-09-27)

Second scénario de fuite, distinct de la mort du proxy : le listener
SOCKS5 de la lane reste joignable alors que le circuit est détruit —
**proxy joignable ≠ circuit disponible** (une sonde TCP du watchdog
proxy seule ne démontrait pas la protection).

- `kill_switch.rs` → engagements **scopés**
  (`engage_scoped`/`release_scoped`, portées `proxy`/`circuits`/
  `manuel`) : le switch reste engagé tant qu'une portée signale une
  panne ; `reason()` liste les portées actives. Le watchdog proxy du
  moteur utilise la portée `proxy`.
- `tribler-tunnel::TunnelCommunity::watch_circuits()` : canal
  `watch` incrémenté à chaque mutation d'état de circuit (création,
  hop ajouté → `READY`, `DESTROY` reçu → `on_destroy`). Le polling
  seul ratait une transition `READY → détruit` plus rapide qu'un
  tick — la détection est désormais événementielle (réaction en ms).
- `tribler-core::ipv8_stack::spawn_circuit_watchdog` : une tâche par
  lane anonyme, **fail-closed dès la création** — la portée
  `circuits` est engagée avant le premier `READY` (un `add`/`resume`
  prématuré est refusé par `guard()` plutôt que d'attendre un CONNECT
  voué à l'échec), réengagée dès que `ready_circuits_of_hops(hops)`
  devient vide et relâchée dès qu'un circuit `READY` au bon nombre de
  sauts revient — même prédicat que la sélection de circuits données
  du SOCKS5, donc une lane 2 sauts n'est pas désarmée par un circuit
  1 saut ; tick de 5 s en filet de sécurité. Arrêtée à
  `Ipv8Stack::stop`.
- `TunnelCommunity::data_rx` : mpsc mono-consommateur → **broadcast
  multi-abonnés** — sans cela, la première lane créée accaparait le
  retour des cellules `data` et les lanes suivantes (« `data_rx` déjà
  consommé ») ne recevaient jamais de données. Chaque `Socks5Server`
  filtre par sa `return_map` ; le retard (`Lagged`) est traité comme
  une perte UDP **et logué en `warn`** (reste observable : une
  saturation du canal ne doit pas ressembler à une panne de circuit).
  Isolation prouvée par `socks5_two_lanes_isolated_returns` : deux
  lanes (1 et 2 sauts) actives simultanément sur la même community,
  chacune ne reçoit que les réponses de ses circuits.
- Test `crates/tribler-core/tests/circuit_death.rs` (~5 s) : portée
  `circuits` engagée dès la création de la lane → vrai circuit 1 saut
  vers un relais autonome → échange de données prouvé (`UDP ASSOCIATE`
  SOCKS5 → cellules `data` → `exit_data` → echo UDP) → la lane à
  2 sauts, elle, reste engagée (évaluation par lane) → le relais
  envoie un vrai `DESTROY` cell → portée `circuits` réengagée
  immédiatement **pendant que le listener SOCKS5 accepte encore les
  connexions** → fenêtre morte bornée : zéro datagramme vers le
  serveur d'écho (seule sortie possible du flux) et aucune réponse
  encapsulée — aucune fuite directe, le serveur SOCKS5 n'a pas de
  chemin de sortie hors tunnel → nouveau circuit → désarmement →
  reprise de l'écho. Le scénario « proxy mort » reste couvert par
  `kill_switch_midtransfer`.

## Durcissement étapes 13/16 — test de fuite en plein transfert (2026-09-27)

- `crates/tribler-bittorrent/tests/kill_switch_midtransfer.rs` :
  stub SOCKS5 pilotable (RFC 1928 CONNECT, relais bridé ~400 Ko/s,
  `kill()` ferme le listener **et** coupe les flux établis) +
  downloader rqbit dont tout le trafic pair passe par le proxy.
- Prouvé : transfert en cours → mort du proxy → watchdog engage le
  kill switch (≤ 12 s) → **progression gelée et téléchargement non
  terminé alors que le seeder reste joignable en direct** (toute
  fuite de repli direct l'aurait fini — or rqbit court-circuite sur
  `proxy_config`, vérifié à la source : `StreamConnector::connect`
  propage l'échec sans repli TCP/uTP) → `resume` refusé pendant la
  panne → proxy restauré → watchdog relâche → pause/resume force la
  reconnexion → téléchargement complété, contenu identique.
- Durée ~25 s (pause/resume court-circuite le backoff de reconnexion
  rqbit après rétablissement).

## Étape 11 — interop discovery new-style + punctures prouvée (2026-09-27)

- **Banc** `scripts/interop_discovery.ps1` +
  `examples/discovery_interop_node.rs` +
  `scripts/interop/py_discovery_node.py` (vrai `DiscoveryCommunity`
  pyipv8, endpoints enregistreurs des deux côtés).
- **`INTEROP DISCOVERY OK`**, 10 flags dans les deux sens :
  - Rust → Python : `send_introduction_request` émet un vrai `234`
    (adresse introduite new-style) → Python décode
    (`PY_RECV_NEW_INTRO_REQ`) et répond `233` → `RUST_NEW_INTRO_OK` ;
    `send_puncture_request` émet `232`/`250` (non signés, `dist`
    inclus) → Python décode (`PY_RECV_{NEW,OLD}_PUNCTURE_REQ`) et
    répond `231`/`249` → `RUST_{NEW,OLD}_PUNCTURE_OK`.
  - Python → Rust : `create_introduction_request(new_style=True)`
    (`234`) → Rust répond `233` → Python décode + vérifie la
    signature (`PY_NEW_INTRO_RESP_OK`) ; `create_puncture_request`
    `232`/`250` → Rust répond `231`/`249` vers `lan_walker` (même IP
    WAN loopback, règle `on_puncture_request`) →
    `PY_{NEW,OLD}_PUNCTURE_OK`.
- **Corrections de fidélité `Network::add_verified`** (révélées par
  le banc) : l'adresse du pair est désormais inscrite dans
  `_all_addresses` (`WalkableAddress(b"", None, False)`) comme le
  fait `add_verified_peer` pyipv8 ; un pair à adresse blacklistée
  inconnue n'est **pas** vérifié ; un pair déjà connu absorbe
  l'adresse et le flag `new_style_intro` (objet partagé Python —
  sans cela le flag posé par un `234` reçu était perdu).
- **API ajoutée** : `DiscoveryCommunity::send_puncture_request`
  (équivalent `endpoint.send(create_puncture_request(...))`),
  compteurs-observables `intro_request_count`/`intro_response_count`/
  `puncture_count` (équivalents des hooks `introduction_*_callback`/
  `on_puncture`).
- Robustesse du banc : retries Python sur le premier `234` (course de
  démarrage UDP), timeouts par phase côté Rust (un échec ne cascade
  plus sur les phases suivantes).

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
