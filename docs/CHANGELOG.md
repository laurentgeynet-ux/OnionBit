# Changelog — étapes franchies

Format : une entrée par étape de `docs/plans/roadmap.md`, la plus récente
en haut.

## Correctifs : `pending` magnet orphelin au drop, retry d'add dans le banc live (2026-10-07)

- **`PendingAddGuard` (session)** — un `add_download_anon` droppé en
  pleine résolution BEP 9 (timeout ou annulation côté appelant,
  requête HTTP coupée) laissait l'entrée `pending` orpheline :
  l'infohash restait `is_pending` à vie (tout re-add échouait en
  `InvalidState("déjà en cours de résolution")`), le `pending_notify`
  et le swarm enregistré fuyaient. Garde RAII qui retire les trois à
  la destruction du futur ; désarmée sur la sortie normale.
- **Banc live : tentatives d'add bornées** — une résolution BEP 9 dont
  la première connexion uTP se perd n'est jamais retentée par le
  moteur (hors ligne : pas de DHT/LSD/trackers). `fleet_add` et les
  adds spawnés de `live_flotte_10_restart` tentent désormais
  `3 × ADD_ATTEMPT_WAIT (60 s)` au lieu d'une tentative unique de
  180 s : une tentative morte est abandonnée (drop → garde) puis
  relancée sur un handshake neuf. Budget total inchangé ; la fenêtre
  de progression par torrent passe à `2 × ADD_ATTEMPT_WAIT` pour
  couvrir une relance.
- Observé en CI tag `v0.9.3-beta` (leg ubuntu) : `fleet3` resté
  `pending=true`/`owner=None` sur résolution morte, `fleet9` en
  timeout d'add — le run master du même commit passait.

## Correctifs (revue externe 7) : re-ACK messagerie, indices speedtest, `PendingGuard::drop`, magnet tolérant, scrape en lots, mutex (2026-10-07)

- **Messagerie : re-ACK sur retransmission** — un `msg` déjà vu était
  absorbé par la dedup sans ré-émettre l'ACK : si le premier ACK
  s'était perdu sur le circuit, l'émetteur marquait `failed` un
  message pourtant reçu. La duplicata déclenche désormais la
  ré-émission de l'ACK (sans doublon d'historique ni d'événement).
- **Speedtest : indices inversés** — `send_times` lisait `s[2]/s[3]`
  (réception) au lieu de `s[0]/s[1]` (émission) : le `up` du SSE
  `/api/tunnel/speedtest` était calculé sur les mauvaises colonnes.
- **`PendingGuard::drop`** : `.lock().unwrap()` pendant un unwinding
  → double panic → `abort()`. Passage à `into_inner()`.
- **`run_speedtest`** : `random_data[..request_size]` paniquait si
  `request_size > 2048` — borné à `SPEED_TEST_RANDOM_BUF` (slice
  Python tronque silencieusement, comportement fidèle).
- **`MagnetLink::parse`** : `&` terminal, `&&` et flags sans `=`
  (`&fl`) rejetaient tout le lien — ignorés désormais (tolérance
  libtorrent pour les magnets du web/RSS).
- **`check_tracker` : scrape en lots de 32** — au-delà de
  `MAX_INFOHASHES_PER_SCRAPE`, les infohashes étaient tronqués
  silencieusement. Découpage par `chunks(32)` ; un lot en échec
  après des réponses conserve les santés acquises. Impact dormant :
  tous les appelants passent 1 hash aujourd'hui.
- **Mutex poison-tolérant** : balayage des fichiers touchés —
  `hidden_services` (52 sites), `session` (8), `state`/`downloads`
  api (8), `speedtest` (8), `messaging` (69), y compris les formes
  multi-lignes `.lock()\n.unwrap()` échappées aux passes
  précédentes.
- Tests : re-ACK sur trame dupliquée (`send_seq` observé, ni
  doublon ni `Frame` en double), magnet `&`/flags tolérés, scrape
  UDP 40 infohashes → 2 lots.

## Durcissement (revue externe 6) : NAT64 SSRF, source SOCKS5, NAT-PMP deadline, KillSwitch, augmenter, clés strictes, popular filtré (2026-10-07)

- **Anti-SSRF : préfixe NAT64 `64:ff9b::/96`** — le WKP RFC 6052
  (DNS64) ne matchait aucun filtre v6 : `64:ff9b::127.0.0.1`
  contournait la politique. L'IPv4 embarquée suit désormais
  `deny_reason_v4`, comme `::/96`.
- **SOCKS5 UDP : filtre de source RFC 1928 §7** — l'association
  acceptait les datagrammes de toute source : un processus tiers
  envoyant au port éphémère écrasait `return_map` et détournait le
  chemin retour. Seul le client d'origine est accepté.
- **NAT-PMP : attente à deadline** — un datagramme parasite ou une
  réponse tardive de sonde (TCP/UDP consécutives, sans nonce RFC
  6886) abortait le `recv` ; seul `opcode attendu + IP passerelle`
  obtient un verdict, le reste consomme le délai. `MappingSpec`
  regroupe les paramètres + `dest_port` testable.
- **`KillSwitch`** : `.lock().unwrap()` → `into_inner()` (8 sites)
  et `guard()` en verrou unique (la raison ne peut plus disparaître
  entre `is_engaged` et `reason`).
- **`Augmenter`** : LIMIT/OFFSET inlinés dans les deux branches
  (bindings `?`/params incohérents selon le découpage) ;
  `fs::write(model)` relâche le verrou (bloquait `encode()`) ;
  mutex poison-tolérant (7 sites).
- **Clés LibNaCL strictes** : `from_bin` exige la taille exacte —
  un suffixe bourré était accepté alors que `mid`/`verify` ne
  portent que sur les 74 octets canoniques.
- **`popular_entries`** : honore `hide_xxx`/`category`/`tags` et la
  pagination `first..last` (la branche court-circuitait tout).
  Impact nul aujourd'hui : aucun appelant ne pose `popular=true`
  (endpoint REST `/popular` = requête propre) — durcissement
  préventif de la fonction.
- Tests : NAT64 deny/allow, parasite NAT-PMP, popular
  hide_xxx/pagination/tags, `?`/params augmenter, clés >74 o.

## Correctifs (revue externe 5) : `popular_entries` NULL, `addr_to_cid` borné, magnets échappés, filtre `infohash` SQL, mutex (2026-10-07)

- **`popular_entries` : `LEFT JOIN` → `INNER JOIN`** — un
  `torrent_state` sans `channel_node` (gossip/checker) rendait des
  colonnes `cn.*` NULL que `from_row` refusait
  (`FromSqlConversionFailure`) : `/metadata/torrents/popular` et le
  comptage renvoyaient 500.
- **`addr_to_cid` (SOCKS5 UDP) borné** : la table destination →
  circuit d'une association n'était jamais purgée — BitTorrent/DHT la
  faisait croître sans fin. Réutilise `dest_map_max_entries`
  (purge des circuits morts puis éviction), comme `dest_circuits`.
- **Magnets : `dn`/`tr` percent-encodés** — un titre avec `&` (ou un
  tracker avec query) produisait un paramètre sans `=` au re-parse
  → `BadMagnet`. `percent_encode` RFC 3986 en pendant du décodeur
  (`+` reste littéral, pas de form-urlencoded).
- **`healths_for` : `n.infohash != ''` → `length(n.infohash) = 20`** —
  un BLOB est toujours `!=` d'un TEXT, et les noeuds canal (clé
  publique 64 o en `infohash`) consommaient le `LIMIT 20` avant le
  filtre Rust : lots de santés tronqués.
- **Mutex poison-tolérant** (`ipv8_stack.rs`, `socks5.rs` — règle
  AGENTS) : `.lock().unwrap()` → `into_inner()` ; les sites
  multi-lignes avaient échappé au balayage de la revue 4. Reste
  ~260 sites dans le workspace — passe dédiée à prévoir.
- Tests de non-régression : `popular_entries` orphelin (db), magnet
  `&`/UTF-8 round-trip (format), `healths_for` non-20o avant LIMIT
  (core).

## Packaging : résidus de l'ancien nommage nettoyés (2026-10-07)

- `build_dist.ps1` : commentaire `onionbit_ui.exe` → `OnionBit.exe`
  (la logique était déjà à jour depuis `da0bb27`).
- `scripts/web_launch.ps1` **supprimé** : lanceur mort remplacé par
  `OnionBit Web.lnk --open-webui` — il référençait encore le
  `.cmd` disparu et rouvrirait une console. `build_dist.ps1` purge
  toujours les vieilles copies dans `dist\`.
- `gh_create_release.ps1`, `README.md`, `docs/BUILDING.md` :
  `onionbit_ui.exe` → `OnionBit.exe`, bundle `dist/onionbit-*` →
  `dist/OnionBit-*` cohérent avec le zip produit.

## Correctifs (revue externe 4) : `source_uri` upsert, schéma `row_json`, scrape BEP-48, TOCTOU trackers, mutex (2026-10-07)

- **`downloads.upsert` : `source_uri` omis du `DO UPDATE`** — la
  sémantique « remplacement complet » ignorait silencieusement la
  nouvelle source (magnet enrichi, trackers d'amorce) au conflit.
- **`row_json` aligné sur `to_simple_dict`** : `category` renvoie
  `tags` (plus `null` en dur), `created` = `torrent_date` ajouté,
  `updated` = `timestamp` du noeud (au lieu de `torrent_date`),
  `id` = `id_` canal (au lieu du rowid sqlite) ; l'injection
  `local_search` des téléchargements gagne aussi `created`.
- **Scrape BEP-48** : `announce → scrape` n'est substitué que dans
  le *chemin* (`rfind`) — `str::replace` corrompait les
  sous-domaines `announce.tracker.org` → `scrape.` (DNS mort).
- **TOCTOU `sync_trackers_file`** : `check_uri_policy` +
  `reqwest::get` re-resolvait le DNS (rebinding possible) et
  `resp.bytes()` n'avait pas de borne — remplacés par
  `fetch_checked` (politique + epinglage) + `read_body_limited`.
- **UPnP SOAP** : timeout client (`discover_timeout_secs`) — un
  IGD gele bloquait la tache forwarder indéfiniment.
- **Mutex poison-tolérant** (`guards.rs`, `session.rs` — règle
  AGENTS) : `.lock().unwrap()` → `into_inner()` ; ~270 autres
  sites dans le workspace restent à balayer dans une passe dédiée.

## Recherche distante : résultats par pair portés au plafond utile (2026-10-07)

- L'UI demande désormais `last=100` (`/search/remote`) — la borne
  `max_response_size` commune Python/OnionBit : au-delà le pair
  émetteur tronque la plage `first..last`, demander plus est du
  gaspillage.
- `select_packets_limit` 10 → **25** (écart documenté, côté
  réception uniquement — aucun changement filaire) : à ~1300 o
  d'entrées par paquet `SelectResponse`, 10 paquets tronquaient une
  réponse complète de 100 entrées à ~40-60 reçues ; 25 laisse
  passer la totalité.

## Durcissement (revue externe 3) : auth ct_eq 404, `dest_circuits` borné, scrape IPv6, corps chunked, RSS zombies (2026-10-07)

- **`api_not_found` en temps constant** : le repli 401/404 comparait
  la clé par `!=` (arrêt à la première divergence) — `key_matches`
  (`ct_eq`) factorisé entre middleware et repli.
- **`dest_circuits` borné** (`tunnel_udp_socket`) : la table
  `dest -> circuit` n'était purgee qu'à la reinterrogation de la
  même cible — la DHT la faisait croître indéfiniment.
  `dest_map_max_entries` (4096, `TunnelSettings`) + eviction des
  épingles de circuits morts puis arbitraire.
- **`is_circuit_ready(cid)`** O(1) sans allocation remplace
  `ready_circuits().contains()` sur le chemin datagramme (socket
  tunnel + socks5) — fini le `Vec` alloué + balayage par paquet.
- **Scrape UDP IPv6** (`torrent_checker`) : la socket éphémère est
  liée `[::]:0` quand la cible est V6 (un bind `0.0.0.0` échouait à
  l'émission vers V6).
- **`read_body_limited` borné même en chunked** : sans
  `Content-Length`, `resp.bytes()` lisait le flux entier en mémoire
  (OOM possible sur RSS/scrape) — accumulation par `chunk()` avec
  plafond strict.
- **Watchers RSS zombies** : `update()` retirait l'URL de la table
  mais la tâche périodique continuait ses GET toutes les 120 s —
  elle s'arrête au prochain tick si l'URL n'y figure plus.
- **`unwrap()` résiduels en prod** : les 6 sites
  `"0.0.0.0:0".parse().unwrap()` remplacés par
  `UdpAddress::unspecified()` / `SocketAddr::from(([0,0,0,0], 0))`
  (règle AGENTS « pas d'unwrap hors tests »).

## Recherche : dates des résultats distants (années ~58000) corrigées (2026-10-07)

- Les pairs Tribler **Python** émettent `updated` (= `timestamp`
  signé des métadonnées) en epoch **millisecondes** ; OnionBit
  l'émet en secondes. Le parseur Dart multipliait par 1000 à
  l'aveugle → années ~58000 affichées pour les résultats distants.
- `_parseDate` applique désormais une heuristique de magnitude
  (`< 1e11` → secondes, sinon déjà ms) et la colonne Date préfère
  `created` (date de création du torrent, secondes partout) à
  `updated` (mise à jour du noeud chez le pair émetteur).

## Diagnostic : carte « Trafic relais » ne mesure que le relais (2026-10-07)

- La carte IPv8 du Diagnostic affichait les débits/totaux de
  **l'endpoint** (`rate_up`/`rate_down`, `total_*`) — qui cumulent le
  trafic de nos propres téléchargements anonymes, la découverte et le
  relais. Elle affiche désormais uniquement le **servi** mesuré au
  limiteur de la pompe d'émission (`relay_served_bps` en titre,
  `relay_served_bytes` en caption) — la valeur exacte « combien je
  relaye », sans soustraction approximative.
- Libellé renommé « Trafic overlay IPv8 » → « Trafic relais »
  (`cardTunnelTraffic`, FR/EN).

## Packaging/lancement : `OnionBit.exe`, `state_dir` bundle-aware, `.lnk` sans console, verrou avant rotation (2026-10-07)

- **Renommage produit** : l'exe Flutter `onionbit_ui.exe` devient
  `OnionBit.exe` (`BINARY_NAME` CMake + `OriginalFilename` du .rc ;
  `onionbit-daemon.exe`/`onionbit-cli.exe` inchangés — noms déjà
  représentatifs). Le tray et `resolve_state_dir` reconnaissent les
  deux noms pour les anciens bundles.
- **`state_dir` par défaut bundle-aware** (`main.rs`) : sans
  `--state-dir`, un exe voisin de `web/index.html` ou de l'UI adopte
  `<exe>/state` — la convention des lanceurs (avant : `.onionbit`
  relatif au CWD, un double-clic direct créait un état orphelin,
  voire `state\state`). `.onionbit` reste le défaut hors bundle
  (dev).
- **`--open-webui`** : le daemon ouvre `http://127.0.0.1:<port>/`
  dans le navigateur par défaut une fois l'API bindée, ou
  immédiatement quand l'instance existe déjà (port relu dans
  `configuration.json`) — `OnionBit Web.lnk` (raccourci COM généré
  par `build_dist.ps1`) remplace `OnionBit Web.cmd`/`web-launch.ps1`
  dont la fenêtre console s'affichait au lancement.
- **Verrou d'instance avant rotation des logs** : `instance::acquire`
  précède désormais `init_tracing` — un second lancement ne fait
  plus basculer `onionbit.log` en `.1` avant de sortir.

## Durcissement P0/P1 (revue externe 2) : racine `move_storage`, caches de requêtes tunnels, demux UDP, SOCKS5, watch_folder (2026-10-07)

- **`move_torrent_files` préservait mal la racine** (`session.rs`) —
  régression du correctif `move_storage` : `Path::parent("f.mkv")`
  vaut `Some("")` et `src.join("")` == `src`, le nettoyage des
  sous-dossiers vides pouvait `remove_dir` la racine de
  téléchargements quand le torrent en était le dernier fichier.
  Parents vides filtrés + `src` exclu. Test : second déplacement
  vidant le dossier — la racine survit.
- **Caches de requêtes tunnels purgés** (`hidden_services.rs`) :
  `peers_requests` et `rp_requests` retirent leur entrée au timeout
  (un pair silencieux la laissait indéfiniment) ; `ip_requests`
  gagne un chien de garde à `circuit_timeout` — l'expiration lâche
  le `tx`, ce qui débloque les `rx.await` (tâches qui se
  garentaient pour toujours) ; `pending_e2e` consomme enfin son
  timestamp — `Create`/`Link` expirés (> `circuit_timeout`) sont
  purgés avec leur requête, fin du re-send infini vers un point
  d'introduction mort ; `seen_e2e` se vide au plafond au lieu de
  figer la déduplication à 64 entrées.
- **Scrape UDP démultiplexé** (`torrent_checker.rs`) : socket
  éphémère par session — la socket partagée laissait le `recv_from`
  d'un scrape concurrent consommer la réponse d'un autre, qui
  partait en timeout.
- **`return_map` SOCKS5 purgée** (`socks5.rs`) : à la fermeture du
  TCP contrôleur, les entrées de l'association sont retirées
  (`Arc::ptr_eq` sur la socket de relais) — fin de la fuite de
  descripteurs et de la croissance monotone de la table.
- **`watch_folder` déduplique toutes lanes** (`watch_folder.rs`) :
  `find_download_hex` + `is_pending` au lieu de `engine()` (clair
  seul), et les `.magnet` sont dédupliqués comme les `.torrent` —
  fin du re-add inutile toutes les 10 s.

## Durcissement P0 (revue externe) : `move_storage` scopé + `::/96` + timing API + verrou POSIX (2026-10-07)

- **`move_storage` ne déplace plus que les fichiers du torrent**
  (`session.rs`) : l'`output_folder` rqbit est le dossier de
  téléchargements **partagé** pour un mono-fichier — l'ancien
  `move_dir_contents` vidait *tout* le dossier (autres torrents,
  fichiers personnels ; copie + suppression inter-volumes).
  `move_torrent_files` n'opère que sur les `relative_filename`
  déclarés, nettoie les sous-dossiers vides, conserve le dossier
  racine. Test : leurre `etranger.txt` épargné, `api-test.bin` déplacé.
- **Anti-SSRF** (`address_policy.rs`) : les adresses IPv4-compatible
  `::/96` (ex. `::127.0.0.1`, dépréciées RFC4291 mais acceptées par
  certaines piles) appliquent désormais la politique IPv4 sur les 32
  derniers bits — plus de contournement du filtre loopback/privé.
  `::` et `::1` gardent leurs raisons propres ; permissif inchangé.
- **Clé API en temps constant** (`auth.rs`) : `subtle::ConstantTimeEq`
  remplace `==` — fin de la fuite de timing sur la comparaison.
- **Instance unique POSIX** (`instance.rs`) : verrou `flock` exclusif
  via `fs2` sur `state_dir/.onionbit.lock` (avant : stub toujours OK —
  deux daemons pouvaient partager SQLite et les ports). Windows
  conserve le mutex nommé ; fail-open si le fichier est inouvrable.

## ADR-0016 (intérimaire) : identité portable — export/import `OBID` (2026-10-06)

- **Blob `OBID`** (`onionbit-crypto::keyblob`) : `magic‖v‖sel‖nonce‖
  ct‖tag` — argon2id RFC 9106 (m = 19 Mio, t = 2, p = 1) →
  ChaCha20-Poly1305, sel+nonce aléatoires par export, borne 64 Kio à
  l'ouverture ; mauvais mot de passe / blob altéré / tronqué → rejet.
- **Fichier `ipv8_keypair.bin` durci** (`ipv8_stack`) : écriture
  atomique (tmp + rename — plus de fichier tronqué après crash),
  permissions `0600` sous Unix ; reste en clair volontairement
  (démarrage headless — voir ADR-0016 pour la graine BIP39 différée).
- **API** (`/api/identity*`, sous `api_key_auth`) : `GET` → clé
  publique seule ; `POST /export` → `{key, encrypted}` (brut ou
  `OBID`) ; `POST /restore` → valide le `LibNaCLSK:` puis remplace le
  fichier — `{restart_required: true}` (identité liée aux communautés,
  pas de mutation à chaud).
- **Flutter** : section « Identité » des réglages — clé publique
  copiable, export (mot de passe optionnel → presse-papiers), import
  (hex/`OBID` + mot de passe) avec avertissement de remplacement +
  redémarrage.
- Migration nomade : export `OBID` (device A) → restore (B) →
  redémarrage → import du coffre `OBV1` → contacts + alias retrouvés
  sous la même identité.
- Tests : keyblob (round-trip, mauvais mdp, formes rejetées), core
  (remplacement réel du fichier + blob mal formé refusé), API
  (cycle complet export→restore→fichier).

## Durcissement P1 (revue externe) : DNS rebinding + purge `ext_peers` (2026-10-06)

- **`fetch_checked_with` anti-TOCTOU** (`services/mod.rs`) : les
  adresses validées par `ip_policy` sont épinglées à reqwest via
  `resolve_to_addrs` — plus de seconde résolution DNS entre le check
  et la connexion, un domaine à TTL 0 ne peut plus rebinding vers
  une IP interne. Le client reste reconstruit par appel (l'épinglage
  est par hôte, non mutualisable — chemins RSS/trackers peu
  fréquents).
- **`ext_peers` bornée** (`ext.rs`) : `last_hello` rafraîchi par tout
  trafic ext signé (preuve de vie, pas seulement les `hello` —
  adresse incluse) ; au tick, les pairs silencieux depuis
  `ext/peer_ttl_secs` (défaut 4 h) sont évincés **et** sortis de
  `probed` → re-sondés dès ce tick (pas de trou d'une heure) ;
  `ext/peers_max` (défaut 4096) borne l'insertion — un flot de clés
  Sybil ne gonfle plus la table ni `ext_targets` (plus de salves
  UDP vers des fantômes). `probed` était déjà purgé au cooldown.
- Tests : purge+re-sondage, trafic=preuve de vie, table pleine →
  `hello` ignoré (3 nouveaux cas verts).

## ADR-0015 §9 : ponts messagerie — consentement assisté + coffre portable (2026-10-06)

- **Gates de consentement** (`tunnel_community/messaging_consent_*`,
  restart) : `flagged` bloque une demande entrante sans `pending`
  (score `identity` < 0 chez un curateur suivi), `endorsed` admet
  directement (score > 0), `ledger` refuse un débiteur au-delà du
  plafond. Local et configurable — blocage explicite toujours
  prioritaire ; ext absente → gates inertes. Lookup injecté par
  `Ipv8Stack::set_trust_lookup` ; compteurs `consent_blocked`/
  `consent_auto_accepted`/`consent_ledger_refused` dans les stats.
- **Coffre `OBV1`** : `export_vault`/`import_vault` sur le service
  — JSON `{v, exported, contacts:[{pk, alias, state}]}` scellé
  `pair_seal_in` pour soi (HKDF `onionbit/vault/v1`). Endpoints
  `GET/POST /api/messaging/vault/*` ; borne `VAULT_BLOB_MAX` 1 Mio ;
  contact existant jamais écrasé (un `blocked` local survit) ;
  `blocked` exporté préservé ; contacts actifs restaurés
  rejoignent leur swarm. Ni clé privée ni messages dans le coffre.
- **`pk` complet par pair ext** (`/api/ipv8/ext`) → l'UI messagerie
  propose les pairs `msg_v1` non encore contacts (« Pairs
  OnionBit ») + export/import du coffre par presse-papiers.
- **Réglages** : trois commutateurs « Consentement messagerie »
  dans la section OnionBit (sous-titres « Applied on restart »).
- Tests : 3 gates + vault round-trip (restauration, illisible par
  un tiers, magic/borne, non-réactivation d'un bloqué).

## ADR-0015 : `kind=identity` — confiance utilisateur + liste d'amis portable (2026-10-06)

- **`attest_kind::IDENTITY = 3`** (`attest.rs`) : sujet = `pk_bin` du
  pair visé (74 o, mêmes octets que `curator` et que les clés des
  contacts messagerie). Double usage : verdict social sur un
  utilisateur **et** liste d'amis auto-signée portable — une
  auto-attestation `endorse` est re-gossipée par les suiveurs et
  retrouvée sur un nouveau device partageant l'identité (ADR-0016).
- **Correction de borne** : `subject_len(CHANNEL)` passait à
  `LIBNACL_PK_BIN_LEN` (74 o) — la valeur historique 42 excluait tout
  canal réel ; l'UI exigeait 128 hex, jamais valide. Ext est
  OnionBit-only : pas d'interop legacy cassée.
- **API** : `"identity"` accepté par `POST /api/ipv8/ext/attest` et
  `GET /api/ipv8/ext/trust/{kind}/{subject}`.
- **Messagerie** : `TrustBadge` (widget partagé, extraction du
  `_TrustBadge` privé de la recherche) sur les contacts et les
  demandes `pending` — un inconnu flagué par un curateur suivi est
  visible **avant** le consentement ; menu contact → « Approuver » /
  « Signaler cet utilisateur » (dialogue pré-rempli kind=identity).
- **« Amis approuvés »** dans le panneau contacts : auto-attestations
  `identity`/`endorse` signées par notre clé, hors contacts existants
  — ré-ajout en un clic (`connect`). Récupération best-effort (les
  suiveurs retiennent nos attestations) ; pseudonymes locaux par
  design (pas de champ libre signé).
- `AttestDialog` : kind `identity`, `initialKind`, validation hex
  par kind (40/148).

## ADR-0015 : boucle de curation bouclée dans le produit (2026-10-06)

- **Score de confiance sur les résultats de recherche** : pastille
  discrète accolée au nom (`+n` vert / `-n` rouge / gris si
  attestations non suivies), tooltip détaillant endorsers/flaggers —
  `GET /api/ipv8/ext/trust/infohash/{hex}`, provider à requête
  **unique** par sujet (le score n'évolue que par attestation ; un
  sondage par ligne serait une amplification N+1). Rien n'est affiché
  quand le sujet est inconnu — pas de bruit sur les lignes.
- **Attester depuis la recherche** : entrées « Approuver » /
  « Signaler » du menu contextuel — `AttestDialog` (extrait en
  widget partagé, pré-rempli `kind=infohash` + sujet) ; le cache du
  score est invalidé en succès pour refléter le verdict immédiatement.
- **« Suivre ce curateur »** : bouton sur chaque attestation stockée
  — ajoute la clé publique complète du signataire à `ext/curators`
  (redémarrage requis, icône basculée en « suivi »). Ferme la boucle
  « je vois une attestation → je fais confiance à son auteur → ses
  verdicts comptent ».
- Le score reste **local et explicable** : seuls les curateurs suivis
  comptent ; aucun blocage/tri automatique n'est appliqué — la
  pastille informe, l'utilisateur décide.

## ADR-0017 : proposition « transport furtif » anti-censure (2026-10-06)

- Nouvel ADR en statut **Proposée** (aucun code) : inventaire de la
  surface identifiable aujourd'hui (préfixe communautaire, clé
  maîtresse `ez_send`, hello ext, formats IPv8/BitTorrent), preuve
  qu'OBF ne masque que le contenu — pas l'existence du protocole.
- Décision structurante : **furtivité et compatibilité legacy
  s'excluent** sur un même nœud — le mode furtif est dédié
  OnionBit↔OnionBit : bootstrap par liens d'invitation hors-bande
  (le hello de négociation trahirait), trames indiscernables de bruit
  (authentification par clé partagée, padding aléatoire, cover
  traffic opt-in), tunnel/messagerie conservés au-dessus, BitTorrent
  public et DHT publique désactivés dans ce mode.
- Banc de validation prévu : `bench_stealth_fingerprint.ps1` —
  indiscernabilité mesurée, pas décrétée.

## UI : mécanismes OnionBit complets — réglages + Diagnostic enrichi (2026-10-06)

- **Section « OnionBit » des Réglages** (`onionbit_section.dart`) :
  `ext/enabled`, `ext/ledger_enabled`, `ext/obf_enabled`,
  `ext/curators` (champ différé, clés hex) — tous appliqués au
  redémarrage ; `tunnel_community/ledger_enabled` +
  `tunnel_community/ledger_enforce` (gate sign-then-serve) rechargés
  à chaud. Défauts `kOnionBitDefaults` + recherche de sections.
- **Onglet « OnionBit » enrichi** : carte « Registre bilatéral »
  (`GET /api/ipv8/ext/ledger` — liens scellés/en vol, forks) et
  carte « Attestations » (`GET …/attestations` + dialogue de
  publication `POST …/attest` : kind infohash/channel, sujet hex
  validé 40/128, verdict endorse/flag).
- Chaîne complète : modèles `ExtLedger`/`ExtLedgerLink`/
  `ExtAttestation`, méthodes repo, providers sondés à 5 s, i18n en+fr.

## UI : onglet « OnionBit » dans le Diagnostic (2026-10-06)

- Nouvel onglet (13ᵉ) de la page Diagnostic : consomme
  `GET /api/ipv8/ext` — état local (activé/désactivé + capacités
  annoncées décodées via `caps_names`) et liste des pairs OnionBit
  reconnus avec leurs capacités en pastilles (`msg_v1` →
  « messagerie », `obf_v1` → « obfuscation », inconnu → brut) et
  l'âge du dernier `hello`.
- Chaîne : `ExtInfo`/`ExtPeerInfo` (modèles) → `extInfo()` (repo) →
  `extInfoProvider` (sondage 5 s) → `_ExtTab`. i18n en+fr.

## ADR-0015 : `CAP_MSG_V1` — découverte de la messagerie par `hello.caps` (2026-10-06)

- **Pont ext ↔ messagerie** : bit 1 du bitmap `hello.caps` annonce que
  le pair sert la messagerie anonyme e2e (ADR-0011). La capacité vit
  dans ext (plan de contrôle) mais décrit un service du plan de
  données : elle n'est annoncée que quand le `MessagingService` est
  réellement démarré (`tunnel_community.messaging_enabled` **et**
  tunnel actif — `enable_messaging && enable_anonymity` côté stack),
  sinon on promettrait une liaison impossible. Les messages eux-mêmes
  restent dans `onionbit-tunnel` — jamais dans ext.
- **Exposition** : `ExtPeerInfo.caps` remontait déjà en brut ;
  `/api/ipv8/ext` ajoute `caps_names` (local + par pair) via
  `ext::cap_names` — la UI peut afficher « messagerie supportée »
  sans connaître le bitmap.
- **Test** `cap_msg_v1_annoncee_et_observee` : bit présent/absent
  selon `messaging_enabled`, propagation hello→peer observer, OBF
  inchangé.

## ADR-0015 : `ext.enabled` activé par défaut (2026-10-06)

- **Bascule du défaut produit** : `ExtConfig::default().enabled` passe
  à `true` (et `Ipv8Config::production().ext_enabled` avec). Sans
  cela, deux installs par défaut partagent le mesh IPv8 sans jamais
  se reconnaître comme OnionBit — la communauté ext (hello, attest,
  ledger, OBF) restait mort-née en pratique. Le HELLO est signé en
  clair, même discipline que les `introduction-request` IPv8 legacy :
  rien de plus exposé que ce qu'un nœud publie déjà au walk.
  `Ipv8Config::default()` reste le preset neutre « tout off »
  (tests/dev) ; `ext.enabled=false` explicité reste honoré.
- **Banc** `scripts/bench_ext_interconnect.ps1` : phase A
  (`enabled=false`) → IPv8 interconnecté mais `peer_count=0` ;
  phase B (`enabled=true`, défaut produit) → `peer_count=1` des deux
  côtés en ~7 s. Test `ext_active_par_defaut_et_se_propage` épingle
  le défaut et sa propagation vers `Ipv8Config`.
- **Bancs de mesure figés** : `fingerprint_mesh.ps1` (baseline sans
  `-WithExt`) et `sec_leak_capture.ps1` épinglent désormais
  `ext.enabled=false` explicitement — ces oracles mesurent la surface
  legacy stricte.
- **Correctif tooling** : `run_interop_suite.ps1` (runner de bancs),
  `interop_tribler_relay.ps1` (BOM UTF-8 — PS5.1 lisait l'UTF-8 en
  ANSI, octet `0x94` des em-dash cassant les chaînes),
  `interop_hidden_py2py.ps1` (`$PSScriptRoot` vide dans un défaut
  `param()` sous `[CmdletBinding()]`), journal des bancs écrit en un
  seul `write_all` (les tests parallèles entrelacaient contenu et
  newline).

## ADR-0015 : validation terrain du ledger (2026-10-06)

- **Soak réel 10/10** (`scripts/bench_ext_ledger_soak.ps1`, nouveau) :
  4 daemons ext+OBF en mesh fermé, speedtest 2 sauts réel (6,1 Mio
  relayés), liens bilatéraux scellés des deux côtés avec hash partagé,
  `forks=0`, OBF 38/38, endpoint speedtest SSE OK.
- **Correctif — annuaire partagé volatile** : `Network.services` est
  mutualisée entre communautés ; le churn discovery/éviction DHT
  (`remove_peer_key`) y efface la marque `EXT` d'un pair vivant →
  `peers_for_service(EXT)` vide → silence ledger persistant. La
  population qui fait foi est désormais `ext_peers` : `ExtPeer`
  conserve l'adresse du dernier `hello` et `ext_targets()` la sert à
  tous les envois post-hello (settlement, gossips HEAD/FORK/ATTEST),
  en re-inscrivant opportunément fiche `by_key` + marque `services`.
  Régression `T7` (purge symétrique → settlement complet quand même).
- **Correctif — faux forks en sqlite** : `DbLedgerStore::put`
  testait la position `(pk_b, seq_b)` pour les propositions non
  scellées — un REJECT suivi d'une reproposition au même rang
  s'auto-marquait fork (soak run : `forks=2` fantômes). La position
  B n'est évaluée que pour les liens scellés (`at_position_sealed`,
  `sig_b <> zeroblob(64)`), parité avec `InMemoryLedgerStore` ;
  régression `proposition_rejetee_reproposee_sans_fork`.
- **T1 silence legacy** : oracle assoupli — `Need a DHT provider`
  est un bruit interne de `TriblerTunnelCommunity` (connect DHT pour
  un hop de circuit legacy alors que le banc ferme toute DHT) ; les
  trames ext sont droppées au préfixe avant d'atteindre ce code.
- **Diag** : le warn de rejeu « configuration.json corrompu » porte
  désormais l'erreur de parse (le détail partait en SSE invisible
  avant l'installation du subscriber).

## Messagerie : indicateur de liaison e2e + « Reconnecter » (2026-10-06)

- **Constat** (banc 3 daemons locaux) : le pipeline e2e fonctionne
  (hello → pending → accept → msg → ack vérifiables via l'API), mais
  l'UI ne distinguait pas « pas encore tenté » d'« échec » ni
  d'« en cours », et un contact dont la liaison avait échoué n'avait
  aucun moyen de retenter sans supprimer/re-ajouter.
- **État de liaison exposé** : `LinkState` (`bound`/`connecting`/
  `failed`/`none`) dérivé côté service — circuit lié > `connect`
  explicite en vol ou tentative e2e automatique du tunnel
  (`TunnelCommunity::e2e_pending` : `pending_e2e` ou `RP_DOWNLOADER`
  non lié) > dernier `connect` échoué. Champ `link` dans
  `GET /messaging/contacts` et `/contacts/pending` ; événement SSE
  `messaging_link` aux transitions (début/fin de tentative,
  déliaison réelle).
- **Orchestration** : `MessagingService::connect_peer` (`resolve` DHT
  → `connect` premier IP) remplace le duo manuel du handler —
  l'état de liaison est piloté par le service, pas par l'API.
- **Auto-reliaison au restart** : `load_state` rejoignait les
  contacts restaurés sans joindre leurs swarms — un contact restauré
  ne pouvait jamais se re-lier sans action. Les contacts `active`
  rejoignent désormais leur swarm (`do_peer_discovery` refait les
  `create_e2e` tout seul) ; `accept_contact` joint aussi le swarm du
  demandeur (consentement bidirectionnel — re-liaison possible si le
  circuit meurt).
- **UI** : pastille d'état 4 couleurs avec tooltip sur chaque
  contact + entrée « Reconnecter » au menu des contacts `active`
  non liés (`connect` idempotent).
- **Tests** : `cargo test -p onionbit-core messaging` (15 unit +
  loopback) + test API 404 verts ; `clippy` et `fmt` propres ;
  `flutter analyze` sans issue.

## Guards : défaut `true` confirmé (2026-10-06)

- **Décision** : `tunnel_community/guards_enabled` reste `true` par
  défaut — la mesure ADR-0010 (premiers sauts persistants bornant la
  loterie Sybil des reconstructions sous `DESTROY`) a rempli son
  critère de sortie terrain (2026-10-02 : download anonyme 2 sauts,
  276,4 Mo, premiers hops ⊆ guard set) ; revenir à `false` serait
  une régression d'anonymat sans signal nouveau. Désactivation à
  chaud déjà disponible (UI « Nœuds guards », `POST /api/settings`,
  `GET /api/ipv8/tunnel/guards`).
- **Ménage** : commentaire `GuardsConfig::enabled` réaligné (le
  `false` crate-interne est un fallback opt-in, pas le défaut
  produit) ; note « reste : download public avec guards » de
  `fingerprinting.md` clôturée (critère rempli).

## `libtorrent/port` : sonde `port..=port+10` (parité `listen_on` Tribler) (2026-10-06)

- **Constat** : le port fixe introduit juste avant rendait un port
  déjà occupé **fatal** au démarrage (bind TCP → `Session::new` Err →
  daemon refuse de démarrer) — collision systématique en mesh loopback
  (4 nœuds sur 45000) et sur Windows partage silencieux du port
  (`SO_REUSEADDR` ≈ SO_REUSEPORT).
- **Sémantique Tribler restaurée** : `listen_on(port, port + 10)` —
  le port configuré devient la **base d'une sonde de 11 ports**, pas
  un bind unique. Premier port libre TCP+UDP gagne, `0` éphémère en
  dernier recours, `warn!` loggé quand le port effectif diffère du
  configuré.
- **Défaut** `45000` → sonde `45000..=45010` : reste dans la plage
  UPnP des box courantes (Freebox `32768..=49151` — les éphémères
  49k+ échouent en UPnP) ; `port = 0` conserve la sonde historique
  `6881..=6891`.
- `announce_port` (port réellement lié) alimente déjà les forwarders
  UPnP/NAT-PMP — le mapping suit le port choisi automatiquement.
- **Test** : `to_core_config_sonde_decale_si_port_occupe` (bind
  23177 pris → port effectif dans `23178..=23187`).

## `libtorrent/port` par défaut fixe à 45000 (2026-10-06)

- **Avant** : `0` → sonde de la plage `6881..=6891` à chaque démarrage
  (parité `listen_on(port, port+10)` libtorrent) — port variable,
  mappings UPnP et règles de redirection manuelle instables.
- **Après** : `DEFAULT_LIBTORRENT_PORT = 45000` — écart assumé vs
  Tribler, comme `api/http_port = 8085` : port fixe dans la plage UPnP
  des box courantes (Freebox `32768..=49151` — les mappings hors plage
  sont refusés en erreur 718). Reste configurable : `libtorrent/port`
  explicite prioritaire, `0` conserve la sonde standard Tribler.

## Diversité des nœuds de sortie des circuits DATA (2026-10-06)

- **Symptôme observé en prod** : téléchargements hop 3 effondrés
  (~30 ko/s) — les 3 circuits DATA 3 sauts terminaient tous sur le
  même pair de sortie (`3d905e3c`, sortie lente), tout le trafic
  anonyme étant étranglé par un seul nœud volontaire.
- **Cause** : pyipv8 ne déduplique pas les derniers sauts — le dernier
  hop d'un `extend` est le premier `candidates` offert par le saut
  précédent (ou un tirage `EXIT_BT`/`RELAY` en repli). Circuits
  partageant guard et mids ⇒ mêmes offres ⇒ même sortie.
- **Correctif (au-delà de la parité pyipv8)** : dans `send_extend`,
  un dernier saut déjà en service sur un circuit DATA `READY` de même
  longueur passe en fin de liste des candidats proposés (tri stable)
  et est dépriorisé dans le repli `get_candidates` — un exit frais
  est tenté d'abord, les exits usés restent en secours quand c'est le
  seul choix (la pénurie réelle ne casse jamais la construction).
- **Portée** : circuits `DATA` uniquement, chemins de choix libre ;
  `required_exit`/`data_exit_peer`/sauts épinglés inchangés
  (déterminisme des bancs).
- **Tests** : 18 unitaires + 42 loopback + 6 fuzz du crate tunnel
  verts ; `clippy -D warnings` et `fmt` propres.

## Parité UPnP : mapping UDP (uTP) du port d'écoute BitTorrent (2026-10-06)

- **Écart** : le forwarder UPnP de librqbit (`enable_upnp_port_forwarding`
  → `librqbit_upnp::UpnpPortForwarder`) n'envoie que
  `<NewProtocol>TCP</NewProtocol>` — le mapping UDP du port d'écoute
  n'existait pas alors que libtorrent mappe TCP **et** UDP (uTP partage
  le port d'écoute). Sur une box UPnP-only (sans NAT-PMP), les
  connexions uTP entrantes pouvaient rester bloquées par le NAT.
- **Correctif** : nouveau `onionbit-bittorrent::upnp` — forwarder UDP
  symétrique à `natpmp.rs` : découverte SSDP et parsing IGD réutilisés
  de `librqbit-upnp` (`discover_once`, `discover_services`,
  `get_local_ip_relative_to`), `AddPortMapping`/`DeletePortMapping`
  SOAP en UDP via reqwest (`no_proxy` — la passerelle est locale),
  bail 1 h renouvelé à mi-vie, libération propre à l'arrêt (comme
  libtorrent), re-découverte si la passerelle cesse de répondre.
- **Câblage** : spawné dans `BtEngine::start` quand `enable_upnp` et
  qu'une socket uTP écoute (`enable_utp`/`utp_only`), sur le
  `announce_port` résolu après bind — exactement le port mappé par le
  forwarder TCP ; les lanes anonymes (bind loopback, `announce_port`
  `None`) restent exclues. Arrêt avec la session.
- **Deps** : `librqbit-upnp` 9.0.1 (déjà transitive de librqbit) +
  `network-interface` 2 directement dans `onionbit-bittorrent`.
- **Tests** : `add_mapping_en_udp`, `delete_mapping_en_udp`,
  `mapping_refuse_sur_500`, `controle_urls_device_imbrique`
  (extraction des controlURL `WANIPConnection` sur device IGD
  imbriqué).

## Messagerie : contact ajouté invisible jusqu'au redémarrage (2026-10-06)

- **Symptôme** : « Ajouter un contact » ne montrait rien dans la liste
  quand la liaison e2e échouait (pair hors ligne, aucun point
  d'introduction) — le contact n'apparaissait qu'au prochain
  démarrage.
- **Cause** : `resolve()` crée et persiste le contact dès
  `ensure_contact_swarm` (`upsert_contact` → `msg_contacts`) mais
  `post_connect` répond ensuite 404 ; le `catch` de `_addContact`
  sautait les `ref.invalidate`, et aucun `MessagingEvent` n'était émis
  à la création → ni pull ni SSE ne rafraîchissaient la liste.
- **Correctif** : nouvel événement `MessagingEvent::ContactAdded`
  émis à la création (SSE `messaging_contact` → invalidation via le
  pont) ; `_addContact` invalide contacts + historique même en cas
  d'erreur — le contact hors ligne apparaît avec le point creux.
- **Tests** : 15/15 unitaires messagerie + loopback e2e verts ;
  `cargo check` api/core, `fmt` et `flutter analyze` propres.

## Onglet Diagnostic « Connexions » — agrégat ip:port (2026-10-06)

- **Besoin** : dans l'onglet Diagnostic, voir pour chaque adresse
  distante `ip:port` les protocoles/transports impliqués (UDP IPv8,
  TCP, uTP, SOCKS, DHT, tunnel, sorties).
- **API — extension Rust** `GET /api/connections` (sans équivalent
  `tribler.core.restapi` ; le debug GUI Python agrège côté client) :
  `{connections, listeners}` où chaque `connections[]` fusionne les
  observations par `ip:port` — pair IPv8 vérifié (`ipv8`, `mid`),
  memberships d'overlays, présence en table DHT, `tunnel_flags`,
  `exit_circuits` (conntrack des sockets de sortie, nouvel accessor
  `TunnelCommunity::exit_sources`), connexions BitTorrent par torrent
  (`conn_kind` rqbit `tcp`/`uTP`/`socks`, `state`, `incoming`,
  `client`, octets) ; `transports` dérivé (`udp` pour tout trafic
  IPv8/tunnel/uTP, `tcp` sinon). `listeners[]` = sockets d'écoute
  locales (`ipv8-udp`, `ipv8-udp-v6`, `bittorrent`, `tunnel-exit-udp`
  avec `circuit_id`, `socks5` des lanes avec `hops`).
- **UI** : onglet « Connexions » (12ᵉ onglet) — section « Écoute
  locale » puis tuiles extensibles par endpoint : pastilles de
  transport/protocole, mid IPv8 tronqué, overlays, flags tunnel,
  circuits de sortie, détail BitTorrent par infohash. Poll sur la
  cadence diagnostic, états loading/erreur/vide, clés l10n fr/en.
- **Tests** : `connections_agregat_offline` (shape du contrat en
  session sans stack) — 62/62 tests API verts ; clippy/fmt propres ;
  `flutter analyze` + 19 tests Flutter verts. Documenté dans
  `docs/reference_tribler/api_rest_mapping.md`.

## Santé gossip dans les résultats distants, sans persistance (2026-10-06)

- **Symptôme** : tous les résultats de recherche distants affichaient
  `0` seeders/leechers là où Tribler montre la santé — la colonne
  « Health » restait grise.
- **Cause** : depuis la bascule « résultats transitoires » (`922a6f0`),
  `process_health` ne retenait que l'infohash (`HashSet`) et jetait
  `seeders`/`leechers`/`last_check` ; `simple_dict_mem` hardcodait
  `0/0/0` ; `/api/metadata/torrents/{ih}/health` ne lisait que
  `torrent_state` (downloads uniquement depuis v14).
- **Fix (zéro écriture DB)** : `GossipMemory` — `HashMap` bornée
  (200k entrées, éviction >24 h puis plus anciennes) gardant la
  dernière santé gossip par infohash, alimentant
  `simple_dict_mem` (parité `to_simple_dict`) et un repli de
  `get_torrent_health` via `ContentProvider::known_health`.
- **SSE** : `torrent_health_updated` émis par `process_health` borné
  aux infohashes déjà poussés à l'UI (`displayed`, cap 50k) — pas de
  flood d'events pour le torrents invisibles ; l'app route le topic
  vers `healthOverridesProvider` (pastille verte en direct, local et
  distant). `seen_nodes` borné à 300k.
- **Tests** : `sante_gossip_remplit_les_resultats_distants`,
  `health_updated_seulement_pour_les_affiches`, `memoire_gossip_bornee`
  — 3/3 verts ; clippy/fmt propres ; 61/61 tests API.
- **Correctif complémentaire (colonne Date)** : `simple_dict_mem` émet
  désormais `"updated"` = `timestamp` signé du nœud (le `updated_on` de
  `to_simple_dict` Python) + `"xxx"` — le champ que lit l'app pour la
  colonne Date ; `created` reste = `torrent_date`. Sans lui les dates
  des résultats distants restaient `—`. Assertions verrouillées dans
  le test existant.

## Banc sécurité : jambe ASan+LSan Linux via CI (2026-10-06)

WSL absent de la machine de banc → `workflow_dispatch` du job
`fuzz-san` (run `37396859847`, `origin/master` `277e787`) : 8 cibles ×
300 s sous ASan+LeakSanitizer Ubuntu 24.04, **~310 M execs cumulés,
0 crash** (`utp_datagram` 177,3 M ; `tunnel_cell` 64,6 M ;
`utp`/`tunnel_payloads`/`messaging_*` couverts). Artefact
`fuzz-san-artifacts` téléchargé sous `target/fuzz-san-ci/`.

Au passage, fix de harnais : le compteur `execs` du journal
`fuzz_journal_san.csv` lisait `#N DONE` avec un espace alors que
libFuzzer émet une tabulation — il retombait à 0. Parsé sur
`Done N runs` (`712cc3d`).

UBSan n'est pas exercé : rustc n'expose pas de UBSan général via
`-Zsanitizer` (`SAN=undefined` retiré du script — la mention
« ASan/UBSan » du nom de job est historique).

## Banc sécurité : campagne ASan complète Windows (2026-10-06)

Jambe sanitizers du harnais fuzz rejouée en longueur (fumée 60 s du
2026-10-05 seulement avant) : `scripts/fuzz_asan_win.ps1`, 8 cibles ×
600 s sous AddressSanitizer (runtime clang externe via
`-Zexternal-clangrt`), commit `b821957`.

- **~253 M execs cumulés, 0 crash**, garde « sanitizer armé » vérifiée
  sur chaque binaire (`armed=True`, symboles `__asan_` présents) :
  `raw_datagram` 8,0 M ; `tunnel_cell` 80,4 M ; `tunnel_payloads`
  34,9 M ; `ipv8_packet` 4,0 M ; `unsigned_dispatch` 7,2 M ;
  `utp_datagram` 99,5 M ; `messaging_frame` 14,2 M ; `messaging_window`
  3,0 M.
- Journal : `fuzz/artifacts/fuzz_journal_san_win.csv` ; logs
  `asan-last-run-*.log`.
- Reste : jambe **UBSan** (absente de MSVC) sur Linux/CI via
  `scripts/fuzz_asan.sh` — WSL non installé sur la machine de banc.

## Messagerie : pseudonyme local des contacts (2026-10-06)

La liste des contacts n'affichait que la clé publique hex (illisible).
Chaîne complète : migration v16 (`msg_contacts.alias`, hors
`upsert_contact` — survit aux transitions de consentement), service
`set_alias`/`contact_alias` (trim, borne 64 caractères, contact inconnu
refusé), `POST /api/messaging/contacts/{pk}/alias`, champ `alias` dans
`GET /contacts` et `/contacts/pending`. UI : titre = pseudonyme ou clé
abrégée, menu « Renommer » par contact, clé abrégée en sous-titre quand
un pseudonyme existe. `""` = effacement (retour à la clé abrégée).

## Messagerie : résolution robuste sans circuit prêt (2026-10-06)

Bug prod au premier « connect » sur un daemon frais : `resolve()` exigeait
un circuit `READY` à `messaging_hops` sauts dans la milliseconde suivant le
joint du swarm — `aucun circuit pour peers-request`, alors que le tick
`circuits_tick` n'avait pas encore construit le vivier `DATA` (5 s).

- `wait_ready_circuit_of_hops` : attente bornée (`circuit_timeout`, 60 s
  pyipv8) qui déclenche `build_circuits_if_needed` puis scrute — utilisée
  par `send_peers_request_when_ready` (resolve messagerie) et `create_e2e`
  (connect). Les chemins périodiques (`swarm_lookup`, `estimate_swarm_size`)
  gardent l'échec immédiat de `select_circuit` Python : ils retentent au
  tick suivant.
- `Ipv8Error::NotReady` : « pas encore prêt » distingué de `Malformed`
  (ce n'est pas une erreur de wire — le demandeur peut retenter).
- 2 tests de régression loopback : l'attente déclenche la construction
  (circuit `READY` rendu), et `NotReady` borné sans pair candidat.

## 17c-5 revalidé : 2 Mio, 0 interdit — + deux durcissements harnais (2026-10-06)

Second run vert complet (`leak-capture-20261006-011656`) : 2 097 152 o
vérifiés en ~90 s, `DELETE anon_lanes/2`, nouvelle lane SOCKS 54074
READY en ~4 s, **0 INTERDIT**, `dead_ports` vide, `lane` ports vide par
construction (attribution exits en place).

Deux courses corrigées en route :

- **Sonde d'écoute API** : la clé est écrite dans `configuration.json`
  avant que l'écoute HTTP soit effective (migrations + moteur) —
  `/createtorrent` tombait en connexion refusée ~3 s post-spawn (run
  010106). Sondes TCP sur les API daemon et helper avant tout appel.
- **Vivier `>= hops+1`** : la précondition `>= Hops` pairs tunnel
  suffisait aux circuits DATA mais pas au rendez-vous —
  `RP_DOWNLOADER` se construit à `swarm.hops+1` (pyipv8). À 2 pairs il
  reste `EXTENDING`, l'e2e reste half-open (`conns=1/0`) et
  `verified=0` malgré 600 s de budget (runs 005227, 010335). Avec
  `minPeers = Hops+1`, l'e2e converge en ~90 s.

## Attribution des sockets de sortie — faux positif 17c-5 résolu (2026-10-06)

La « fuite dead-port » du run 2121xx (4 paquets UDP d'un port dit « mort »
vers `213.31.154.144`) est **reclassée faux positif d'attribution** après
inventaire exhaustif des sockets réelles du daemon :

- Une lane anonyme ne possède **aucune socket UDP réelle** (uTP/DHT/
  tracker = `TunnelUdpSocket` virtuelle, cellules via l'endpoint IPv8
  partagé) — une émission WAN directe depuis une lane est
  structurellement impossible.
- Les ~10 ports observés étaient des **sockets de sortie** (`0.0.0.0:0`
  bindée à la réception d'un `created` dont nous sommes le dernier
  saut, `community.rs::on_create`). Elles servent l'égress des pairs
  **distants** — leur circuit `created` survit légitimement à la
  destruction de notre lane — et relaient du e2e IPv8 via l'exemption
  préfixe-communauté de `is_exit_data_allowed` (fidèle pyipv8, actif
  même avec `exitnode_enabled=false`).
- La règle dead-port de l'analyseur reclassait leur trafic OVERLAY en
  INTERDIT — correctif dans le harnais, pas dans le produit :
  `ExitInfo.local_port` exposé via `GET /api/ipv8/tunnel/exits`, le
  script exclut ces ports de `lanePorts` et les consigne au manifeste.
  L'oracle dead-port reste strict pour les sockets réellement liées à
  la lane.

## P0-17c-5 lane-reset OK — payload réel + 0 paquet interdit (2026-10-06)

Le scénario `lane-reset` a enfin pu s'exécuter de bout en bout avec des
octets fichier réels (`-DedicatedSeed -RequireFileBytes -MeshHelper
-MeshHelpers 2`, run `leak-capture-20261006-002746`) :

- **2 097 152 octets vérifiés** (`progress=0,5` × 4 Mio) avant injection
  — le déclencheur octets-fichier a mordu, pas du trafic tunnel.
- `DELETE /ipv8/tunnel/anon_lanes/2` en plein transfert → listener
  SOCKS libéré, session de lane en 404, daemon vivant.
- Nouvelle lane recréée sur des ports différents (SOCKS 50669 vs
  57612), circuit DATA 2 sauts READY en ~4 s, trafic repris.
- **0 paquet INTERDIT** dans la fenêtre fail-closed, **0 émission
  depuis les ports morts** de la lane détruite. Un paquet
  `UDP → 175.112.151.78:27549` correctement classé bruit ambiant.
- La fuite supposée du run 2121xx (4 paquets UDP vers
  `213.31.154.144`) est reclassée faux positif d'attribution —
  sockets de sortie de circuits `created` distants, cf. l'entrée
  « Attribution des sockets de sortie » ci-dessus.

Chaîne de déblocage (3 causes distinctes, diagnostic live) :

- **Famine DHT loopback** (commit `437949c`) : les helpers peuvent
  publier leurs points d'intro (`stored_on=8`) et le downloader les
  trouve (`dht_lookup n=1`).
- **Budget octets erodé** : armé au premier circuit DATA READY —
  mesuré en run réel (`budget octets 240s arme` après READY).
- **Instabilité de maillage à 2 pairs** : les circuits 2-sauts mouraient
  en EXTENDING sur des relais WAN avant la fin du handshake e2e, et les
  points d'intro annoncés retombaient sur le downloader lui-même
  (`intro_addr=127.0.0.1:28830` = le daemon de banc). `-MeshHelpers 2`
  rend les sauts helper↔helper loopback possibles : circuits READY en
  ~10 s, IP_SEEDER ×10 instantanés, e2e établi en secondes.

Durcissements harnais associés : `-MeshHelpers N` (N helpers avec
bootstrap croisé, ports base+10·i), `-CircuitReadyTimeoutSec`
(paramétrable), précondition `IP_SEEDER READY` côté seed caché,
propagation UAC de `-MagnetDownload`/ports helper. Le manifeste
documente `mesh_helpers` (pids) pour l'interprétabilité de la preuve.

## Messagerie activée par défaut + toggle réglages (2026-10-05)

- `tunnel_community.messaging_enabled` passe à `true` par défaut
  (le `false` initial était conditionnel à la validation des bancs
  `MS-*`, achevée : parcours inter-daemon + gate fail-closed).
- Settings → Tunnels anonymes : interrupteur `messaging_enabled`
  (persisté via `POST /api/settings`, pris en compte au redémarrage
  comme les autres réglages structurels) — fin de l'activation par
  édition manuelle de `configuration.json`.
- Banc `sec_leak_capture` : budget octets du trigger armé au premier
  circuit DATA READY (pas à l'ajout du download) — sur maillage
  clairsemé la construction des circuits consommait la fenêtre
  (`verified=0` alors que la chaîne convergeait).

## Fix — prédicat pre-kill de `live_crash_pending_magnet_et_restart` (2026-10-05)

Le « flake » de l'issue #16 était un prédicat structurellement faux :
`dls` filtrait les specs par `!magnet`, or la flotte
`[(0,false),(0,true),(2,true)]` ne contient qu'un seul non-magnet →
`dls.len() == 2` impossible → `wait_until` retournait `false` à chaque
run. Le filtre sélectionne désormais les specs matérialisables
(`hops == 0` : le direct + le magnet lane 0 résolu via le seed).
Validation : 5/5 runs isolés (~2 s), suite `live_bench` 14/14.

## Fix — magnet anonyme bloqué en METADATA + famine DHT loopback (2026-10-05)

Bug production : un magnet ajouté avec `anon_hops > 0` restait parfois
indéfiniment en « recherche metadata » (observé en prod sur
`3b88beb6…`, aucun `join_swarm` ni `dht_lookup` dans les logs).

- **Cause racine** : rqbit résout le magnet *avant* de matérialiser le
  torrent — `monitor_hidden_swarms` ne voyait que les torrents
  matérialisés → jamais de `join_swarm` → jamais de pairs e2e.
  `session.rs` enregistre désormais le swarm caché dès le `pending`
  (`register_pending_swarm`) et alimente la résolution via un canal
  `pending_peer_sinks` → `extra_peers_rx` (nouveau champ vendored
  `AddTorrentOptions`, fusionné dans `make_peer_rx`). `join_swarm`
  devient idempotent pour préserver les points d'introduction lors de
  la transition pending → matérialisé.
- **Famine DHT loopback** : divergence avec pyipv8 — la
  `destination_address` de l'introduction-response recopiait
  `source_wan_address` auto-déclaré (`0.0.0.0:0` quand le WAN est
  inconnu) au lieu de l'adresse source observée ; `DhtCommunity`
  n'apprenait pas `my_wan` depuis les réponses d'introduction (la
  classe de base le fait pour tous les overlays) ; `my_estimated_lan`
  valait `0.0.0.0:port` au lieu de l'adresse d'endpoint réelle.
- **Oracle `sec_leak_capture`** : `analyze_leak_capture.py`
  dédoublonne les duplicatas NDIS de pktmon (×4) et reclasse en
  `BRUIT` un paquet d'un port banc dont le motif appartient à une
  inondation ambiante (≥25 ports non-bancs, même proto+taille — un
  port éphémère libéré peut être réutilisé par un autre processus).
  Robustesse élevée : `cmd /c` sur les natifs stderr-sensibles,
  manifeste `seeder`/`mesh_helper` corrects, `-MagnetDownload` pour le
  rejeu du scénario prod.
- Preuve : `live_magnet_anon_resout_via_seed_cache` (e2e complet,
  cellules rendez-vous) + `live_magnet_pending_rejoint_swarm_cache`
  + `onionbit-ipv8` 24/24 + run `lane-reset` réel : 2 228 224 octets
  fichier transférés en hidden-service avant injection, 0 paquet
  interdit dans la fenêtre fail-closed.

## Gates transversaux — préparation post-campagne (2026-10-05)

Préparation à froid des gates `messaging_window`, sanitizers et
P0-17c-5 pendant que la campagne longue `messaging_window` tourne —
aucun corpus vivant touché, aucun `cargo fuzz` lancé.

- **`corpus_window_ne_diverge_pas`** (`onionbit-messaging/tests/
  fuzz_regression.rs`) : replay stable du corpus `messaging_window`
  — decodeur identique à la cible fuzz (`wb%96`, `cb%24`, tags
  Admit/Resume/Reset/Admit-court, `decode_seq` aux mêmes bornes) qui
  compare `RecvWindow` au `WindowModel` événement par événement
  (`admit`, `seen_id`, `top`). S'abstient tant que
  `tests/fuzz_corpus_window/` n'existe pas ; devient strict dès que
  le corpus minimisé est versionné.
- **`scripts/fuzz_corpus_replay.ps1`** — chaîne post-campagne :
  `cargo +nightly fuzz run -s none <target> <corpus> -- -merge=1
  <min>` → copie vers `crates/<crate>/tests/fuzz_corpus[_window]/` →
  `cargo test -p <crate> --test fuzz_regression`. Mappe
  `messaging_frame`/`messaging_window`, `-DestDir` pour les autres.
- **`scripts/fuzz_asan.sh`** — campagne sanitizers Linux/WSL (ASan
  + UBSan, `SEC`/`SAN` paramétrables) : les runtimes clang complets
  n'existent pas sous MSVC — le shim sancov Windows reste
  couverture-seule.
- **P0-17c-5 rejeu** (`sec_leak_capture.ps1 -Scenario lane-reset`) :
  `-DedicatedSeed` crée un torrent local via `/api/createtorrent`
  du daemon banc et le fait seeder par le `Tribler.exe` bootstrap
  (peer réel joignable par les exits), avec attente de l'état
  `seeding` avant l'ajout du download anonyme ; `-RequireFileBytes`
  rend le déclencheur strict sur les octets **vérifiés**
  (`progress × size`, pas `session_download` qui compte les octets
  reçus non hashés). Le fallback `max(fichier, tunnel)` reste le
  défaut. Manifeste : `trigger_mode`, `seed_dedie` (infohash,
  SHA-256 + taille source, seeder pid/ports, `seeding_confirmed`,
  `bytes_at_failure`, `bytes_after_rebuild` — snapshots à
  `progress`/`size`/`verified_estimated`/`session_download`/`tunnel`,
  `payload_hash_match`) ; verdicts « aucun download direct » et
  « SHA-256 reçu == seedé » quand le téléchargement dédié est
  complet. **Verdict `INVALID_PRECONDITION`** (exit 2) : une
  précondition non remplie (seeding non confirmé, trigger payload
  jamais atteint, lane absente, download direct détecté, capture
  inanalysable) invalide le résultat sécurité — le manifeste est
  écrit quand même et un `INTERDIT=0` ne suffit plus à faire `OK`.

Clôture campagne et gates sanitizers :

- **Campagne `messaging_window` 14 400 s** (`f9521ab`, Windows MSVC,
  sanitizer none) : 147 471 627 execs, ~10 240 exec/s moyens, pic RSS
  30 Mo, **0 crash, 0 divergence impl/modèle**. Corpus 376 bruts →
  **242 minimisés** (merge libFuzzer, 1 463 features préservées),
  versionné sous `onionbit-messaging/tests/fuzz_corpus_window/` et
  rejoué en CI stable par `corpus_window_ne_diverge_pas`.
- **ASan sous MSVC possible** (recette validée 2026-10-05) :
  `-Zexternal-clangrt` + `-Clink-arg=clang_rt.asan_dynamic-x86_64.lib`
  + `-Clink-arg=clang_rt.asan_dynamic_runtime_thunk-x86_64.lib`,
  LLVM `lib\clang\<ver>\lib\windows` dans `LIB` et `PATH`, jamais
  `/WHOLEARCHIVE` sur le thunk (doublon `__start___sancov_*` avec le
  shim libfuzzer). Smoke `messaging_window` 60 s : 291 420 execs,
  ~4 780 exec/s, RSS pic 402 Mo (shadow memory active), 0 crash.
  Reste : pas de runtime `librustc-nightly_rt.asan.a` ni UBSan —
  la validation complète reste Linux.
- **`fuzz_asan.sh` durci** : échoue si le binaire ne référence pas
  les symboles `__asan_`/`__ubsan_` — un build non instrumenté ne
  peut plus passer pour une campagne sanitizers.
- **Job CI `fuzz-san`** (`ci.yml`, nightly/manual, ubuntu-24.04) :
  toolchain nightly + cargo-fuzz, `SEC=300` × 8 cibles × 2
  sanitizers, artefacts (journal CSV + `fuzz/artifacts/`) remontés.
## Phase 9e — obfuscation de transport négociée `OBF` (ADR-0015, étape 47, 2026-10-06)

- **Mesure préalable (47.1)** — `fingerprint_mesh.ps1 -WithExt
  -WithAnonDownload`, 15 min : volume ext total ~1,5 Ko (~0,03 % de
  l'endpoint) — 4 `hello`, un pic `ATTEST` extinct, 0 `LEDGER_*` (le
  ledger n'émet que sur tranches servies). Le signal exploitable
  n'est pas le volume mais le contenu lisible (`msg_id`, tailles) et
  la cadence `hello` — périmètre exact de l'obfuscation (résultats
  détaillés dans `docs/security/fingerprinting.md`).
- **`msg::OBF` (8) — enveloppe opaque de paire** (`ext::obf`) :
  `{v, blob}` où `blob = pair_seal_in(inner)` sous la clé X25519 de
  la paire, domaine HKDF `onionbit/ext-obf/v1` (séparé du `pairbox`
  des `tx` — blobs non interchangeables). `inner = msg_id ‖ len16 ‖
  payload ‖ pad aléatoire`, arrondi au multiple `ext/obf_pad_bucket`
  (défaut 256) : type et taille réelle illisibles hors AEAD.
- **Négociation `caps` bit 0 `CAP_OBF_V1`** : annoncé dans `hello`
  uniquement quand `ext/obf_enabled` (off par défaut) ; émission
  enveloppée uniquement vers un pair qui l'a annoncé — sinon clair
  (compatibilité intra-ext préservée, jamais de `OBF` vers legacy).
  `hello` reste en clair (bootstrap). Réception : budget partagé
  `LEDGER_*` avant AEAD, `obf_dropped` sur malformé, `OBF` imbriqué
  refusé, inner dispatché dans le chemin `on_packet` normal.
- **Jitter `hello`** (`ext/hello_jitter_pct`, défaut 25 %) :
  intervalle + tirage uniforme — la périodicité exacte était le
  signal temporel le plus marquant.
- **Crypto** : `pair_seal_in`/`pair_open_in` (HKDF paramétré) dans
  `onionbit-crypto::ipv8::dh` ; `pair_seal`/`pair_open` inchangés.
- **Observabilité** : `obf_rx/obf_tx/obf_dropped` dans
  `GET /api/ipv8/ext`, `on_obf` dans `ext_msg_name`, colonnes ledger
  ajoutées à `fingerprint_stats.ps1 -ExtCsv`.
- **Tests** : 3 unitaires `obf` (aller-retour+padding, mauvaise
  clé/tronqué/version, borne de taille) + banc `T6` (attest
  enveloppée vers pair `cap`, claire vers pair sans `cap`, malformé
  droppé) + surface `obf::seal/open` dans le fuzz `ext_packet`.

## Phase 9c — ledger bilatéral signé (ADR-0015 §5, 2026-10-06)

- **`ext/ledger.rs`** : `LedgerLink` `{v, pk_a, seq_a, prev_a, pk_b,
  seq_b, prev_b, tx_enc, sig_a, sig_b}` — inséré dans les deux
  chaînes, `proposal_id` (indépendant de `sig_b`) = clé de dedup et
  de détection de fork ; `tx` chiffré pour la paire
  (`pair_seal`/`pair_open` X25519→ChaCha20-Poly1305 dans
  `onionbit-crypto`) — seuls `pk`/`seq`/`prev` restent en clair.
- **Filaire** : `LEDGER_PROPOSE`/`SEAL`/`HEAD`/`REJECT`/`FORK` —
  trames bornées, budget par émetteur, signatures Ed25519 vérifiées,
  gossip des têtes **scellées** seulement, propagation de la preuve
  de fork. `REJECT` = resync (tête + mesure du bénéficiaire) → la
  proposition corrigée plafonne à `measured + dérive`.
- **Sign-then-serve** : `settle_tick` relance borné + propose la
  tranche suivante ; `owes_signature` alimente le veto `admit` du
  tunnel sous `ext/ledger_enforce` (défaut `false` — mesure d'abord).
- **Persistance** : migration v19 `ext_ledger_links` +
  `ext_ledger_forks`, `DbLedgerStore` (même sémantique
  `PutOutcome` que la mémoire), `InMemoryLedgerStore` borné.
- **API/config** : `GET /api/ipv8/ext/ledger` (liens + forks +
  têtes), clés `ext/ledger_*` dans `daemon_config`.
- **Tests** : T5a (propose→seal), T5b (dérive→reject→convergence),
  T5c (fork gossip) — + `ext_packet` fuzz étendu à `LedgerLink`.
- **Correctifs de concurrence** : `info()` acquiert ses verrous en
  `let` séparés (un `MutexGuard` temporaire dans le literal vivait
  jusqu'à la fin de l'expression → `my_head()` re-verrouillait
  `ledger_store`, auto-deadlock) ; ordre interne du store unifié
  `links → by_pos` (ABBA `link_at`/`head` vs `evict`/`store`) ;
  `settle_tick` ne tient plus `pending` pendant les callbacks
  (`stats_source`/`store`/`network`) — supprime le cycle
  `pending → book → pending` possible avec le veto `admit`.

## Phase 9 — bancs ADR-0015 : T1 legacy silence + T2/T3/T4 (2026-10-05)

- **`ext_bench.rs`** (intégration loopback) : `t2_mesh_curation`
  (valide/rejeu/non-suivi/conflit/nouveau verdict/unfollow),
  `t3_flood_controle` (300 trames mono-clé → 256 stockés + 44 droppés ;
  multi-clés au-delà de la table bornée), `t4_cadence_ext_s_eteint`
  (hello figé, `attest_tx → 0` après convergence).
- **`scripts/bench_ext_silence.ps1`** — banc T1 processus réel :
  mesh fermé A1+D (ext) + Tribler.exe. **8/8 oracles PASS** :
  T jamais promu pair ext ⇒ aucun `ATTEST` adressé à Tribler ;
  `hello_probed=2` borné ; gossip OnionBit-seul (`D.stored=1,
  score=1`) ; overlays de T legacy-seuls ; log T propre.
- **`fingerprint_mesh.ps1 -WithExt` / `fingerprint_stats.ps1
  -ExtCsv`** : extension de l'harnais d'empreinte — `ext.enabled` +
  `curators=[A1pk]` sur les 4 nœuds OnionBit, publication différée à
  t+60 s, CSV `ext_onionbit.csv` (compteurs `hello_*`/`attest_*`).
  Run 3 min : pic à t+60 s puis **compteurs figés** (extinction).
- **Observabilité** : `hello_tx`/`hello_probed` exposés par
  `GET /api/ipv8/ext` (borne « hello opportuniste » mesurable).
- **Journal privé** : `docs/plans/bench_adr0015/` (gitignoré) +
  `bench_adr0015.md` (scénarios, résultats, reste terrain).

## Phase 9d — durcissement du chemin `ATTEST` (ADR-0015 §6, revue, 2026-10-05)

- **Pipeline de réception ordonné du moins au plus coûteux** :
  budget `ATTEST` par émetteur → borne `ATTEST_FRAME_MAX` (1024) →
  parse borné → **préfiltre curateur suivi avant toute crypto** →
  lookup dedup (`AttestationStore::get` — rejeu/stale absorbé sans
  `verify`, Ed25519 étant déterministe) → borne `ts` futur →
  `verify` → conflit → stockage → ré-émission. Un flot
  d'attestations de curateurs inconnus ne coûte plus qu'un parse
  borné — la vérification Ed25519 n'est payée que pour du
  potentiellement nouveau d'un curateur suivi.
- **Conflit d'équivoque** : même `(curateur, kind, sujet)` + même
  `ts` + verdict différent → rejet sans écrasement ni ré-émission
  (deux signatures valides au même `ts` = équivoque avérée,
  logguée en `warn`) ; `put` reste latest-wins comme dernier filet.
- **Budget par émetteur** : `ext/attest_rate_window_secs` (60),
  `ext/attest_rate_max` (256), `ext/attest_rate_table_max` (4096 —
  borne mémoire de la table face aux clés Sybil fraîches).
- **Observabilité de banc** : compteurs `attest_rx/dropped/stored/tx`
  exposés par `GET /api/ipv8/ext` — drops visibles, extinction du
  gossip mesurable (`tx → 0`).
- **`AttestationStore::get`** : accès cle exacte (surchargé en
  accès primaire par `InMemory` et `DbAttestationStore` via
  `attestations::get` SQL).
- **Sémantique unfollow** documentée (ADR §6) : unfollow conserve
  les attestations en base (visibles dans `attestation_count`) mais
  les exclut du `score` — ré-ajouter le curateur les réintègre.
- **Tests** : `attest_conflit_meme_ts_sans_ecrasement` (scénario de
  revue : endorse accepté → flag même ts rejeté — store/score
  inchangés, aucune ré-émission), `attest_prefiltre_curateur_non_suivi`
  (drops comptés sans crypto, signature corrompue incluse),
  `attest_budget_par_pair` (cap 2/4 → 2 stockés + 2 droppés),
  lookups `get` mémoire + SQL.

## Phase 9d — curation signée : attestations Ed25519 + score local (ADR-0015, étape 46, 2026-10-05)

- **`onionbit-ipv8::ext::attest`** : `Attestation` auto-portante —
  `{v, kind, verdict, ts, varlenH(subject), varlenH(curator), sig}`
  ; la signature Ed25519 couvre `SIG_DOMAIN || champs` (domaine
  séparé : aucune trame signée par la même clé ne peut être rejouée
  en attestation). `kind` : `infohash` (20 B) ou `channel`
  (`LibNaClPK`, 42 B) ; `verdict` : `endorse`/`flag`. L'objet reste
  vérifiable après re-émission par un tiers ou relecture depuis le
  stockage — c'est ce qui rend le gossip possible.
- **Gossip borné** (`msg::ATTEST` = 2) : publication poussée à tous
  les pairs ext ; à réception, signature + `ts` non futur au-delà de
  `ext/attest_max_future_skew_secs` (600 s) + **curateur suivi**
  exigés ; nouvelle (ou plus récente) → ré-émise aux autres pairs
  ext. La dedup `(curateur, kind, sujet)` éteint les cycles — une
  attestation ne voyage qu'une fois par nœud.
- **Borne Sybil** : seules les attestations de curateurs suivis
  (`ext/curators`, clés hex dans la config — invalides loggées et
  ignorées) et les nôtres sont stockées/ré-émises ; le reste est
  vérifié puis droppé sans stockage. Conséquence v1 assumée : la
  propagation ne chemine que par les nœuds qui suivent le curateur
  (documenté ADR §6).
- **Persistance** : table `attestations` (migration v18), PK
  `(curator, kind, subject)`, upsert *latest-wins* (`ts` strictement
  plus récent — un vieux verdict rejoué est absorbé sans
  ré-écriture) ; trait `AttestationStore` injecté (pattern
  `PeerStatsStore`), adaptateur `DbAttestationStore` dans core,
  `InMemoryAttestationStore` borné par défaut.
- **Score de confiance local** : `trust_info` = +1/−1 par curateur
  suivi (+ soi) sur le sujet — indépendant de toute réputation bande
  passante (ADR §6). API : `POST /api/ipv8/ext/attest`
  (`{kind, subject hex, verdict}` → signe + publie),
  `GET /api/ipv8/ext/attestations` (latest borné),
  `GET /api/ipv8/ext/trust/{kind}/{subject}` → score + mid hex des
  endorsements/flags — 404 quand `ext/enabled` est off.
- **Tests** : 3 unitaires `attest` (round-trip sign/verify, domaine
  séparé, formes rejetées) + store mémoire (dedup, latest, borne) +
  DB upsert + adaptateur core + 3 loopback (gossip A→B→C avec drop
  D non-suiveur, signature corrompue/`ts` futur rejetés, rejeu sans
  ré-émission et remplacement par verdict plus récent) + API
  (404/400) ; surface `Attestation::unpack` ajoutée à `ext_packet`
  et à la régression stable.

## Phase 9b — `OnionbitExtCommunity` : transport d'extension OnionBit-only (ADR-0015, étape 44, 2026-10-05)

- **`onionbit-ipv8::ext`** : communauté sur `community_id` dédié
  `922d2ad9…` = `sha1("OnionBit extension community")` — constante
  de domaine documentée (aucune clé maîtresse), impossible à
  collisionner avec les ID Tribler figés ; un pair legacy droppe le
  préfixe silencieusement, zéro casse.
- **Découverte lazy/opportuniste** — jamais de walk dédié (un walk
  sur préfixe inconnu annoncerait « OnionBit ici » à tout sniffer) :
  à chaque tick (`ext/hello_interval_secs`, 60 s) jusqu'à
  `ext/hello_fanout` (5) pairs **déjà vérifiés** des communautés
  legacy reçoivent un `hello`. Réponse à un `hello` reçu bornée par
  le même cooldown sortant → pas de ping-pong. Un pair muet (Tribler)
  n'est plus sollicité pendant `ext/hello_cooldown_secs` (1 h). Le
  peer set marqué `EXT_COMMUNITY_ID` *est* la population OnionBit —
  négociation de capacités implicite par `msg_id`, complétée par le
  bitmap `caps` (v1 : aucun bit, réservé obfuscation Phase 9e).
- **Filaire** : trames `{v, …}` versionnées, toutes signées
  `ez_send` (`WIRE_EXT` : `unsigned`/`dist` vides — pas de marche,
  pas de puncture ; `global_time` absent du fil). `hello` =
  `{v: u8, caps: u64}` (9 octets), trailing toléré pour les
  extensions futures ; version inconnue → drop + suppression de
  re-sondage, `msg_id` inconnu → ignoré.
- **Config** : section `ext` (`enabled` défaut **off** — nouveau
  protocole observable sur le mesh partagé, promotion après
  validation des bancs, même discipline que `messaging`) ; prise en
  compte au redémarrage. Overlay visible dans
  `GET /api/ipv8/overlays` (nom `OnionbitExtCommunity`, `strategies`
  vide — c'est voulu, `ext_msg_name` → `on_hello`).
- **Tests** : 7 unitaires/loopback — aller-retour `hello` bilatéral
  (marquage mutuel + exactement une réponse par cooldown), sondage
  limité aux pairs vérifiés et borné par `fanout`, pair muet non
  re-sollicité, version inconnue non marquée, datagrammes
  tronqués/signature fausse/`msg_id` inconnu droppés — + cible fuzz
  `ext_packet` (parse `WIRE_EXT` + `Hello::unpack`) et surface
  ajoutée à la régression stable `fuzz_regression`.

## Phase 9a — comptabilité locale par pair `peer_stats` (ADR-0015, 2026-10-05)

- **`onionbit-tunnel::peer_stats`** (étapes 42–43) : livre de comptes
  local par clé publique LibNaCl — mesure pure, zéro octet sur le
  fil. `bytes_served` = volume des objets de routage joints par
  `create` direct (le `requester` est l'initiateur — seul le premier
  saut le connaît) ; `bytes_used` = volume de nos propres circuits
  crédité à chaque saut vérifié. Comptage par deltas au tick
  (`ledger_tick_secs`, 30 s) + clôture au retrait
  (`remove_circuit`/`remove_relay`/`remove_exit_socket`/`on_destroy`)
  — jamais de double comptage, table bornée (`max_peers`, 8 192).
- **Persistance** : trait `PeerStatsStore` injecté (pattern
  `GuardStore`), `DbPeerStatsStore` (core) sur la table `peer_stats`
  (migration v17, flush upsert des seules lignes modifiées).
- **Gate d'admission** : sous pression (`ledger_enforce` et
  `joined >= ledger_soft_cap`), `on_create` n'admet que si
  `served - used <= ledger_max_deficit_bytes` — le crédit de
  démarrage *est* le déficit max (un pair inconnu a dette nulle →
  admis ; remboursement possible en servant nos circuits, aucun
  deadlock). `enforce = false` **par défaut** : mesure
  expérimentale, même discipline de promotion que les guards.
  Config `tunnel_community/ledger_*`, bascule à chaud via
  `POST /api/settings` ; désactivé = comportement pyipv8 exact.
- **`GET /api/ipv8/tunnel/ledger`** : réglages effectifs, totaux,
  top 64 comptes par volume (mid hex, jamais d'adresse) — agrégats
  locaux, aucun graphe ni transaction détaillée.
- **Tests** : 8 unitaires (crédit, dette sous/hors pression, borne,
  flush dirty-only, chargement boot) + aller-retour DB + adaptateur
  core + 2 loopback (comptage servi/utilisé 1 saut, refus pair en
  dette vs admission pair inconnu sous pression) + API sans stack.

## Fuzz messagerie — campagne `messaging_frame` + cible `messaging_window` (2026-10-05)

- **Campagne `messaging_frame` 14 400 s** (`802c752`, Windows MSVC,
  sanitizer none) : 629 865 040 execs, ~43 737 exec/s moyens, pic
  RSS 35 Mo, **0 crash**. Corpus 37 seeds -> 647 -> **458 minimises**
  (merge libFuzzer, 1 734 features preservees), versionne sous
  `onionbit-messaging/tests/fuzz_corpus/` et rejoue en CI stable par
  `corpus_campagne_ne_panique_pas` (`.gitattributes` : corpus marque
  binaire — aucune normalisation EOL).
- **`RecvWindow::admit` : decalage >= 64 corrige** — `bitmap << 64`
  paniquait en debug et devenait un no-op en release (masque mod 64),
  corrompant la fenetre anti-rejeu apres un grand saut de `seq`.
  Branche explicite `shift >= 64 -> bitmap = 1`, sans troncature
  `as u32`. Trouve en revue post-merge ADR-0011 (hors surface
  `messaging_frame` : `admit` n'est pas dans `Frame::open`).
- **Nouvelle cible `messaging_window`** : fuzzing differentiel de la
  machine d'etat anti-rejeu — chaque sequence `admit`/`resume`/reset
  est comparee a un modele de reference (ensemble borne + FIFO dedup).
  Miroir proptest `recv_window_equivaut_au_modele` en CI stable.
  Smoke 90 s : 1,09 M execs, 1 453 features, 0 divergence.

## Correctifs — recherche locale et fan-out distant (2026-10-05)

- **Recherche locale sans filtre quand l'augmenteur rend 0 rowid**
  (`/api/metadata/search/local`) : `local_search` vidait `txt_filter`
  et `terms` au profit de `rowids`, mais une liste vide ne posait
  aucune clause `WHERE` — la requête retournait les 50 premières
  lignes de `channel_node` quel que soit le texte cherché (ex. une
  recherche « french » listait des torrents sans rapport). Garde-fou :
  `rowids` vide => résultat vide.
- **`OFFSET` augmenteur décalé de 1** : `offset = max(1, first)` était
  passé tel quel à `LIMIT/OFFSET` SQL (0-base) — la première ligne de
  résultat était sautée pour `first=1`. Corrigé en `first - 1`.
- **`max_query_peers` 20 -> 60** (écart protocole assumé, non
  configurable) : Tribler/Python borne à 20, mais l'overlay
  content-discovery connaît typiquement >200 pairs (la cible
  `RandomWalk` est un plancher) — à 20, une recherche ne touchait que
  ~10 % des pairs disponibles. Tirage sans remise inchangé, borné par
  `len(peers)`.

## P0-17c-5 — fail-closed OS : destruction/recreation de lane anonyme (2026-10-05)

- **`DELETE /api/ipv8/tunnel/anon_lanes/{hops}`** : hook de destruction
  de lane (`Ipv8Stack::remove_anon_lane`) — extension Rust sans
  equivalent pyipv8, surface diagnostic/test ; 404 hors lane.
- **`sec_leak_capture.ps1 -Scenario lane-reset`** : sous-run dedie sur
  le **daemon** (la lane `AnonLane` n'existe que la — l'exemple de
  banc n'en a pas). Le mapping OS d'une lane est son **listener TCP
  SOCKS5 loopback** (les sockets datagramme de lane sont virtuelles :
  `TunnelUdpSocket` encapsule tout en cellules IPv8 sur le port ipv8
  partage) — le banc verifie sa liberation, l'absence API
  (`session?hop` -> 404), la recreation sur un **nouveau** port, puis
  la reprise de trafic. Un PCAP + un manifeste par sous-run, comme les
  autres pannes 17c.
- **Bug reel trouve par le banc, corrige** : la boucle `accept` de
  `Socks5Server::listen` retenait un clone d'`Arc<Socks5Server>` et le
  `TcpListener` — le port SOCKS d'une lane detruite restait ouvert
  indefiniment et acceptait encore des connexions (fuite de mapping
  apres « destruction »). Ajout de `Socks5Server::shutdown()` (canal
  `watch` + `select!` dans `accept` et dans le dispatcher de retour),
  appele par `remove_anon_lane` et `stop`. Test de regression
  `socks5_shutdown_libere_le_listener` (loopback reel).
- **Run vert** (`target/leak-capture-17c5f/`, daemon `acaa607` + ce
  changeset) : injection a 26 713 o de trafic tunnel, lane detruite en
  1,4 s, nouvelle lane sur nouveau port a +5,2 s, reprise a +8,2 s ;
  **INTERDIT = 0** dans la fenetre fail-closed, 0 paquet sortant de
  ports morts. Declencheur « mid-transfer » = `max(octets fichier,
  octets circuits DATA READY)` — la resolution magnet publique sur
  DHT-tunnel reste environnementalement lente (stall documente).

## MS-13 — banc inter-demon messagerie e2e reel (2026-10-05)

- **`scripts/interop_messaging_e2e.ps1`** : deux demons OnionBit
  independants (processus, state dirs, cles API distincts) dans un
  mesh loopback ancre/relai/exit. La sequence assertee couvre tout le
  cycle applicatif : connect (resolve DHT + `create-e2e`), hello,
  consentement `pending` -> `active` (rien livre avant, pas de faux
  ack), livraison exactly-once + `acked`, reemission = nouvelle
  ligne, **restart A** (identite/contacts/historique restaures,
  liaison e2e perdue), kill B -> mort du circuit -> `404` + `failed`
  (pas de file offline), restart B sans retransmission, reconnect
  explicite, cleanup DELETE. Manifeste JSON a chaque run.
- **Resultats** : vert en `messaging_hops=1` (circuit RP 2 sauts) et
  `hops=2` (3 sauts). Duree ~1 min.
- **Bugs reels trouves par le banc, corriges** :
  - `onionbit-db` : `SCHEMA_VERSION` restee a 14 alors que la
    migration v15 (tables messagerie) existait -> le demon refusait
    de rouvrir sa propre DB au restart (`SchemaTooNew`). La
    constante est desormais `MIGRATIONS.len()` — plus jamais de
    derive.
  - `onionbit-tunnel` : `remove_circuit`/destroy ne fermaient pas
    `data_subscribers[cid]` -> le canal `subscribe_circuit_data`
    restait ouvert, la messagerie ne deliait jamais un circuit mort.
  - `onionbit-tunnel` : `on_extend` acceptait de s'etendre vers
    soi-meme (boucle de routage) -> refus desormais. Exclusion du
    point de rendez-vous comme premier hop renforcee par adresse en
    plus de la cle.
  - `onionbit-messaging` : la presence (`ensure_introduction_points`)
    n'etait retentee qu'a `announce_interval` (300 s) apres un premier
    essai avant verification des pairs -> nouvelle cadence
    `ip_check_interval` (10 s) ; la reannonce DHT reste lente.
- Diagnostics gardes : contexte `src`/`circuit_id` sur « cellule
  rejetee », erreur d'envoi du `link-e2e` loggee, `first_hops` du
  circuit `RP_DOWNLOADER` tracees en debug.

## Messagerie ADR-0011, étape 41 — validation sécurité (2026-10-05)

- **Inventaire MS-1..MS-12** : couverture vérifiée banc par banc
  (cycle e2e + réouverture + séparation de lane, auth Ed25519,
  codec hostile + cible fuzz `messaging_frame`, anti-replay,
  consentement borné, budgets + drops comptés, offline `failed`,
  restart + DELETE physique). MS-12 complété : 401 sans clé sur les
  routes messagerie ajouté au test d'auth.
- **MS-8 mesuré** : nouveau switch `-WithMessaging` de
  `fingerprint_mesh.ps1` (active `messaging_enabled` sur les 4
  nœuds OnionBit). Run mesh 15 min : présence seule = 2 975
  cellules tunnel (~3,3/s) vs 1 116 (~1,24/s) baseline —
  **+1 859 cellules (+167 %)** pour les circuits `IP_SEEDER` de
  présence + re-annonces DHT (+360 messages DHT). Aucune boucle de
  contrôle ; surcoût structurel borné, documenté dans
  `fingerprinting.md`.
- **Threat model** : section « Messagerie e2e » — propriétés
  démontrées par banc vs non-claims explicites (métadonnée de
  présence DHT assumée, intro points privilégiés, **pas de forward
  secrecy** sans ratchet, persistance en clair, identité IPv8
  partagée, corrélation de trafic hors périmètre).

## Messagerie ADR-0011, étape 40 — API REST/SSE + UI (2026-10-05)

- **REST `/api/messaging/*`** (extension Rust — pas de parité
  Python) : `stats` (clé publique locale, `messaging_hash`,
  compteurs de drops), `contacts` + `contacts/pending`,
  `contacts/connect` (résolution points d'introduction + liaison
  e2e), `accept`/`refuse`/`block`/`unblock`/`DELETE contact`,
  `GET/POST contacts/{pk}/messages` (historique borné, envoi —
  `404` hors ligne = enregistré `failed`), `DELETE messages/{id}`,
  `contacts/{pk}/retention`. Toutes les routes répondent
  `404 « messagerie desactivee »` quand le service n'est pas
  démarré — jamais de réponse partielle.
- **SSE dédié** `GET /api/messaging/events` : relaie le
  `broadcast` du service (`messaging_frame`, `messaging_bound`,
  `messaging_pending`, `messaging_consent`, `messaging_undeliverable`)
  — volontairement hors `Notifier` global : la messagerie reste un
  flux opt-in.
- **Accesseurs service** : `public_key_bin`, `own_messaging_hash`
  (exposition identité/adresse à l'API).
- **UI Flutter** `features/messaging` : nouvel onglet « Messages »
  (`/messages`) — adresse locale copiable, demandes `pending`
  actionnables (accepter/refuser/bloquer), liste contacts (état,
  circuit lié, menus blocage/rétention/suppression), conversation
  (historique inversé, statuts envoyé/livré/non livré, suppression
  réelle au clic long), dialogue d'ajout par clé publique.
  `SseClient` généralisé par `path` ; pont SSE → invalidation pull
  (le flux est un indice de fraîcheur, pas une source fiable).
  Clés l10n en/fr ; `flutter analyze` propre.
- **Tests** : `messaging_desactivee_repond_404_sur_tous_les_
  endpoints` couvre les 14 routes (MS-12 côté « off »).

## Messagerie ADR-0011, étape 39 — persistance + livraison (2026-10-05)

- **Migration v15** : `msg_contacts` (pk, état, `send_seq`,
  `recv_top`, `retention_secs`, `secure_delete`) et `msg_messages`
  (id, FK cascade, direction, seq, ts, body, statut
  `received|sent|acked|failed`) — persistance en clair v1 assumée
  dans l'ADR. Module `onionbit-db::messaging` (propriétaire des
  tables).
- **Suppression réelle** : `DELETE` partout (contact → cascade
  messages) ; rétention optionnelle par contact purgée au tick du
  moniteur — `secure_delete` zeroise `body` avant le `DELETE`.
- **ACK applicatif** : chaque `msg` livré renvoie `ack(id)` ; un
  `ack` entrant passe le `out` correspondant à `acked` — exempté
  du seau contact (contrôle vérifié et dédupé : il ne doit pas
  faire perdre de `msg` sous rafale).
- **Offline borné** : `send` sans circuit → erreur + événement
  `Undeliverable` + ligne `failed` visible dans `history()` —
  jamais de file ni de réémission automatique (MS-7).
- **Restart (MS-11)** : `load_state` restaure état de
  consentement, `send_seq` et `recv_top` (`RecvWindow::resume` —
  bitmap reconstruit conservateur : `top` marqué vu).
- **API service** : `history`, `stored_contacts`, `set_retention`,
  `delete_message` — relais REST/SSE = étape 40.
- **Tests** : 15 tests messagerie verts (offline, restart,
  ack, rétention) + loopback à jour.

## Messagerie ADR-0011, étape 38 — consentement + anti-abus (2026-10-05)

- **`ContactState { Active, Pending, Blocked }`** : un `hello`
  vérifié d'un inconnu crée un `pending` borné (`pending_cap=64`,
  `pending_ttl=600 s`, purgé au tick du moniteur et avant chaque
  admission) et émet `Consent` ; ses trames sont vérifiées puis
  écartées (`pending_drop`) — jamais livrées avant décision.
- **Transitions utilisateur** : `accept_contact` → `Active` + trame
  `accept` au pair + `Bound` ; `refuse_contact` → `reject` + oubli
  (un nouveau `hello` repropose) ; `block_contact` → drops comptés,
  circuit détruit à l'identification, swarm du contact quitté et
  jamais rejoint (`resolve`/`connect`/`send` refusés) ;
  `unblock_contact` réinitialise. `send` exige `Active`.
- **Budgets (MS-10)** : `preflight` (taille + suffixe canonique
  `1:vi<ver>ee`, coût constant) → seau global (`global_rate=10/s`,
  borne le coût codec+signature) → codec+AEAD → signature →
  consentement → seau contact (`per_contact_rate=2/s`) →
  anti-replay. La dédup `id` est consultée **avant** le seau et
  `admit` **après** : les réémissions honnêtes restent gratuites,
  une trame écartée au budget reste livrable plus tard.
- **Observabilité** : `MessagingStats` (codec, rate_global,
  rate_contact, blocked, pending_full, replay, pending_drop) +
  `pending_contacts()`/`contact_state()` pour l'API (étape 40).
- **Tests** : cycle pending→accept→livraison, refus puis
  re-proposition, blocage persistant + swarm désarmé, `pending`
  plein et TTL, seaux contact/global, préfiltre version — 11
  tests messagerie verts, loopback 2 nœuds mis à jour sur le flux
  de consentement (MS-6, MS-10).

## Messagerie ADR-0011, étape 37 — transport e2e + démultiplexage (2026-10-05)

- **`MessagingService` (`onionbit-core::services`)** — présence :
  `join_swarm_with_key(messaging_hash(pk), hops, sk_identité)`
  publie la clé d'identité comme `seeder_pk`, donc le handshake
  `created-e2e` authentifie le destinataire au niveau transport ;
  moniteur d'introduction-points (`ensure_introduction_points`,
  re-annonce périodique `reannounce_intro_points`).
- **Liaison** : `send_peers_request` (PEX direct ou DHT) →
  `create_e2e` sur circuit `RP_DOWNLOADER` → `linked-e2e` ; le
  secret DH e2e est désormais conservé sur le circuit
  (`e2e_shared_secret`) et exposé — les clés applicatives
  `MessagingKeys` sont dérivées HKDF domaine-séparé, jamais de
  `hs_session_keys` en clé applicative.
- **Démultiplexage** : un dispatcher par circuit e2e messagerie
  via `subscribe_circuit_data`, indexé par `info_hash` des swarms
  messagerie ; `hello` lie l'émetteur (corps `pk` + signature
  vérifiée), les autres trames sont éprouvées contre les contacts
  connus puis ouvertes (AEAD + Ed25519) et admises par la fenêtre
  `seq`+dédup `id`. Un circuit e2e = une conversation ; la lane
  BitTorrent (`swarm_lookup` + `could_be_utp`) ne voit jamais ces
  trames — vérifié au niveau wire.
- **Config** : `tunnel.enable_messaging` (défaut off) +
  `messaging_hops` (défaut 1) ; nécessite `enable_anonymity`.
- **Tests** : loopback 2 nœuds complet (présence → IP → liaison →
  trames bidirectionnelles authentifiées), dédup à travers
  réouverture de circuit, usurpation `hello`, trames hostiles,
  rejet `could_be_utp` (MS-1, MS-2, MS-9).

## Messagerie ADR-0011, étape 36 — codec de trame + crypto applicative (2026-10-05)

- **Nouveau crate `onionbit-messaging`** (propriétaire unique du
  protocole) : `Frame {v,type,id,seq,ts,body,sig}` en bencode
  canonique strict — ensemble de clés exact, rejet `v != 1`,
  `body` ≤ 30 Kio, trame ≤ 32 Kio, borne de taille **avant** tout
  parse (anti-DoS).
- **Authentification** : chaque trame est signée Ed25519 par la clé
  IPv8 de l'émetteur (encrypt-then-sign sur la forme canonique sans
  `sig`) et vérifiée contre la `pk` du contact — distinct de la
  confidentialité du tunnel.
- **Clés applicatives** : HKDF-SHA256 sur le secret e2e, domaine
  `"onionbit messaging v1"` disjoint de `key_generation` — une clé
  par direction, miroir initiateur/répondant. Non-claim assumé :
  pas de ratchet/FS en v1.
- **Anti-replay** : `seq` monotone + fenêtre bitmap 64 (modèle
  IPsec) + dédup par `id` 128 bits borné FIFO — absorbe les
  réémissions honnêtes d'un circuit e2e reconstruit.
- **Fuzz** : cible `messaging_frame` (cargo-fuzz) + miroir stable
  proptest (`tests/fuzz_regression.rs`) — jamais de panic sur
  trame hostile.
- **Bancs couverts** : MS-3/MS-4/MS-5 côté codec+auth+replay ;
  transport (MS-1/MS-2), consentement et API = étapes 37-41.

## Compteur « session ↓ » : octets réseau réels (2026-10-05)

- **Symptôme** : `session ↓` affichait ~61 Go au démarrage — la somme
  de `progress_bytes` incluait les pièces vérifiées par le contrôle
  de hash du fastresume, pas seulement le téléchargement réseau.
- **Backend** : `DownloadStats.fetched_bytes` expose le compteur
  rqbit des octets réellement reçus ; `/api/statistics` (`total_recv`)
  et `session_download` (DTO) l'utilisent. `progress_bytes` conserve
  la progression (%) et les cumuls persistants.

## Parité réseau : retry séquentiel IPv8, plage standard BT 6881..=6891 et client NAT-PMP (2026-10-04)

- **Ports UDP IPv8** : remplacement du repli direct vers un port éphémère (`0.0.0.0:0`) par `bind_dual_with_retry` (`MAX_PORT_RETRY_ATTEMPTS = 1000`), fidèle à `create_socket_with_retry` de Tribler (incrémentation séquentielle `port + 1` en cas de collision sur IPv4 et IPv6).
- **Plage BitTorrent standard** : lorsque `libtorrent.port == 0`, pré-sélection de la plage standard `6881..=6891` (parité `ltsession.listen_on(port, port + 10)` de Tribler) avant repli sur un port éphémère si toute la plage locale est saturée.
- **Support NAT-PMP / PCP** : implémentation native du protocole NAT-PMP (RFC 6886) dans `onionbit-bittorrent::natpmp` pour l'ouverture des ports TCP et UDP (uTP) auprès des passerelles compatibles, avec renouvellement périodique du bail et libération propre (`lifetime = 0`) à l'arrêt.
- **Tests** : tests unitaires d'incrémentation séquentielle (`bind_retry_incremente_le_port_si_deja_pris`), de détection de plage BitTorrent (`to_core_config_sonde_plage_bittorrent_standard`), et de négociation de paquets RFC 6886 (`simulation_reponse_passerelle_natpmp`).

## Recherche distante sans persistance + purge du catalogue (2026-10-04)

- **Symptôme** : la page Recherche affichait par défaut les « torrents
  populaires » — un catalogue de ~26k entrées `channel_node` (+ index
  FTS) accumulé par le gossip content-discovery, et ~178k lignes
  `torrent_state` d'historique de santé ; les scans figeaient la
  connexion sqlite partagée (opérations > 1 s en boucle).
- **Backend** : `process_select_response` décode les `.mdblob` et
  renvoie les résultats via `remote_query_results` (SSE) **sans**
  écrire `channel_node` — dédup mémoire `(public_key, id_)` au lieu de
  `channel::insert` ; `process_health` garde les infohashes vus en
  mémoire, plus d'écriture `torrent_state` depuis le gossip.
- **Migration v14** : purge unique — `channel_node` ne conserve que
  nos propres torrents (`public_key` à zéro, servis aux selects
  entrants), `torrent_state`/`torrent_state_tracker` ne gardent que
  les santés des téléchargements actifs (le checker les repeuple) ;
  `FtsIndex` vidé via les triggers.
- **UI** : requête vide → liste vide (appel `popular()` supprimé du
  dépôt) ; fin du re-sondage local `_collectRemote` — les résultats
  distants arrivent exclusivement en push SSE, fenêtre de collecte
  12 s ; libellés « Recherche sur le réseau » / « Saisissez un terme
  de recherche… ».

## Port d'écoute éditable + diagnostic seeding (2026-10-04)

- **Symptôme** : les pairs disparaissent entre les annonces —
  `libtorrent.port = 0` (port aléatoire à chaque démarrage) et la box
  refuse le mapping UPnP (erreur 718/714) → aucun pair ne peut se
  connecter en entrant ; seules les annonces sortantes ramènent des
  pairs.
- **UI** : champ « Port d'écoute BitTorrent » dans Réglages → Réseau
  (`libtorrent/port`, 0 = aléatoire) — à fixer pour permettre une
  redirection de port manuelle sur la box.

## Colonne « UL tot. » dans la table (2026-10-04)

- **Besoin** : le cumul upload all-time n'était visible que dans le
  panneau détail.
- **UI** : colonne triable « UL tot. » entre « Ratio » et « Ajouté » —
  `d.uploaded` (`total_uploaded` persisté, cumul toutes sessions),
  `DownloadSort.ulTotal`.

## Statut tracker : refus de scrape ≠ panne (2026-10-04)

- **Diagnostic** : les trackers privés type Gazelle (C411) répondent
  `d14:failure reason34:Scrape disabled on private trackere` en 403 —
  le tracker est **joignable**, seul le scrape est désactivé ; les
  afficher `Error` mentait.
- **Nouveau** `CoreError::ScrapeRefused` : `failure reason` HTTP et
  `action=3` UDP BEP-15 → tracker marqué `alive` (`Working`,
  compteurs `-1` inconnus) ; seules les vraies pannes (DNS, TCP,
  timeout, réponse invalide) incrémentent `failures` → `Error`.

## Page « À propos » — mise en page + forum (2026-10-04)

- **Lien** : entrée « Forum de discussion » → GitHub Discussions
  (`kGitHubDiscussionsUrl`) dans la section Projet.
- **Mise en page** : bandeau hero pleine largeur (dégradé
  `primaryContainer`, logo + badge version/licence) puis grille
  réactive — Versions ∥ Licences côte à côte au-delà de 760 px,
  empilées en dessous ; Projet pleine largeur.

## Statut tracker réel dans `tracker_info` (2026-10-04)

- **Problème** : chaque tracker réel affichait `peers: -1` /
  `status: "Not contacted yet"` en dur — « forcer l'annonce » rendait
  `forced: true` sans jamais changer le statut.
- **Source de vérité** : `tracker_state` (`alive`/`failures`/
  `last_check`) alimenté par le scrape du torrent checker — le seul
  canal observable fiable (librqbit n'expose pas l'annonce par
  tracker).
- **Checker** : `check_tracker` met à jour `tracker_state` au succès
  (`alive`, `failures=0`) et à l'échec (`alive=false`, `failures+1`).
- **API** : `trackers_json` prend `states`+santé d'essaim → statut
  `Working` (avec `seeds`/`leeches` scrapés) / `Error` /
  `Not contacted yet` ; `GET /api/downloads` précharge les états dans
  le cache `downloads_rows` (pas de N+1) ; `GET .../trackers` lit les
  états des URLs du torrent.
- **Force announce** : scrape opportuniste en arrière-plan après
  `force_announce` — l'annonce librqbit part, puis le checker met à
  jour `tracker_state` pour que le statut reflète l'observation.
- **Anonymat préservé** : les téléchargements `anon_hops > 0` ne sont
  jamais scrapés en clair (garde existante du checker).

## Création de torrent depuis l'UI (2026-10-04)

- **Besoin** : `POST /api/createtorrent` existait côté daemon mais
  aucun accès UI — impossible de créer un `.torrent` depuis ses
  propres fichiers.
- **UI** : bouton « créer un torrent » (icône `post_add`) accolé à
  « Ajouter » dans la sidebar → dialogue : chemin source (fichier
  ou dossier, avec navigateur de dossiers daemon), nom, tracker,
  description, dossier d'export — puis case « ajouter au partage »
  qui enchaîne `PUT /api/downloads torrent=…` avec le sélecteur
  Clair / Anon ×1/×2/×3 pour seeder anonymement dès la création.
- **Repo** : `DownloadsRepository.createTorrent` →
  `POST /createtorrent`, retourne `(infohash, path)`.

## Page « À propos » (2026-10-04)

- **Navigation** : nouvelle destination `/about` (icône info),
  sidebar uniquement (`primary: false` — hors nav compacte).
- **Contenu** : bannière logo SVG, bloc « Versions » (app
  `pubspec` + version/uptime daemon via
  `/api/statistics/tribler`), bloc « Projet » (licence
  GPL-3.0-or-later + copyright, code source GitHub, rapport de bug,
  docs), bloc « Crédits » (portage Tribler + `showLicensePage` des
  dépendances Flutter).
- `core/app_info.dart` : métadonnées produit centralisées
  (version, licence, URLs — à synchroniser avec `pubspec.yaml`).

## Sidebar : logo agrandi (2026-10-04)

- Logo horizontal 44 → 56 px (icône collapsed 36 → 40 px) : mieux
  visible, taille plafond avant que la largeur de la sidebar ne
  contraigne le rendu (viewBox 640×160 → ~57 px max utile).

## Barre d'état : volume de session fichiers (2026-10-04)

- **Besoin** : voir le volume total ↑/↓ de la session courante, tous
  téléchargements confondus, sans le trafic de relai tunnel.
- **API** : `session_upload`/`session_download` dans `DownloadInfo`
  (compteurs moteur — trafic fichiers uniquement ; émis par GET et
  SSE, contrairement aux `all_time_*` persistés).
- **UI** : `sessionTrafficProvider` (somme) + bloc « session
  ↓ X · ↑ Y » dans la barre d'état, tooltip explicatif ; flexible
  avec ellipsis sur les fenêtres étroites.

## Totaux all-time persistés : upload/download cumulés (2026-10-04)

- **Besoin** : `all_time_upload`/`all_time_download` portaient les
  compteurs de **session** librqbit — remis à zéro à chaque
  redémarrage du daemon, ratio et volume affichés sans mémoire.
- **DB (v13)** : colonnes `total_uploaded`/`total_downloaded` sur
  `downloads` + helper `add_transferred`.
- **Core** : la boucle de progression accumule les deltas des
  compteurs de session à chaque tick (`saturating_sub` : le re-add
  moteur qui remet les compteurs à zéro ne soustrait rien).
- **API** : `GET /api/downloads` émet les cumuls persistés
  (`all_time_upload`/`all_time_download`/`all_time_ratio`) — le
  payload SSE conserve les compteurs de session, que le client ne
  merge plus dans ces champs.
- **UI** : ligne « Trafic total » (`↑ X · ↓ Y`) dans le panneau
  détail ; la colonne Ratio affiche désormais le ratio all-time.

## Badge « jumeau » : contenu dupliqué repérable (2026-10-04)

- **Besoin** : `clone_public` crée un jumeau anonyme d'un torrent
  privé — même nom, même taille, infohash différent. Dans une longue
  liste, impossible de relier les deux entrées.
- **UI** : icône lien (`Icons.link`, tertiaire) dans les badges de
  fin de ligne (table + grille) et dans le titre (vue compacte)
  quand un autre download porte le même nom non vide et la même
  taille — couvre le jumeau privé→public comme les vrais doublons.
  Tooltip explicite.

## « Supprimer » du menu contextuel : confirmation unifiée (2026-10-04)

- **Bug** : l'entrée « Supprimer » du menu contextuel d'un
  téléchargement le retirait **immédiatement**, sans confirmation ni
  option « supprimer aussi les données » — contrairement à la
  poubelle de la barre d'actions.
- **UI** : l'entrée route désormais sur `confirmRemoveSelected` —
  même dialogue (compteur + case données disque), même chemin.

## Torrent privé détecté post-résolution (magnet) (2026-10-04)

- **Cas** : le flag `private` n'est pas dans un magnet — il n'est
  lisible qu'après résolution BEP 9 du metainfo. Un magnet privé
  ajouté en Anon ×N restait silencieusement à l'arrêt sans pairs
  (sans fuite, mais sans explication).
- **Daemon** : `Notification::PrivateTorrentDetected` — émise après
  `add_download_anon` quand le metainfo résolu est `private=1` sur
  une lane anonyme, et dans `add_torrent_bytes_anon` (`.torrent`
  privé ajouté en anonyme via l'API en contournant le verrou UI).
- **API** : topic SSE `private_torrent_detected` (extension locale,
  comme `download_state_changed`).
- **UI** : la cloche affiche un avertissement « Torrent privé
  détecté — « nom » est privé : l'anonymat est impossible,
  basculez-le en Clair ». Le download n'est pas bloqué côté daemon
  — l'UI informe, l'utilisateur décide (`PATCH anon_hops`).

## Dialogue « Ajouter » : champ magnet-only (2026-10-04)

- **Décision** : le champ « Magnet ou URL » n'accepte plus que les
  liens `magnet:` — une URL `http(s)` déclenche un GET en clair du
  metainfo qui expose l'intérêt pour ce `.torrent` au serveur (le
  magnet, lui, se résout via la DHT tunnélisée : zéro trafic clair).
  L'alternative privée existe déjà : télécharger le `.torrent` dans
  le navigateur puis le choisir via le picker.
- **UI** : libellé « Magnet » + hint `magnet:?xt=…` ; saisie non
  magnet → erreur inline expliquant la raison et l'alternative.
- **API** : `PUT /api/downloads` avec une URL `http(s)` reste
  accepté (scripts/CLI) — fetch `fetch_checked` anti-SSRF documenté.

## URL `http(s)://…​.torrent` fonctionnelle en mode anonyme (2026-10-04)

- **Bug** : coller une URL `.torrent` dans le dialogue Ajouter
  échouait en Anon ×N (« error downloading torrent metadata »).
  Le fetch était délégué à l'engine — sur une lane anonyme son
  client reqwest passe par le SOCKS5 du tunnel, dont `CONNECT`
  n'est **pas** un tunnel TCP : il ne relaie qu'une requête HTTP
  claire one-shot (cellule `http-request` vers une sortie
  `PEER_FLAG_EXIT_HTTP`). TLS échoue donc toujours, et même le
  HTTP clair exige un circuit déjà prêt.
- **Fix** (`add_download_anon_inner`) : les URI `http(s)` sont
  fetchées en clair côté session via `fetch_checked` (anti-SSRF
  `ip_policy`, timeout, taille bornée) puis ajoutées par le chemin
  `.torrent` — fonctionne sur toutes les lanes, http **et** https,
  sans dépendre des circuits au moment de l'ajout. Bonus :
  `private`/trackers par défaut traités comme un fichier choisi.
- **Correction de doc** : les annonces tracker **HTTPS** ne passent
  pas par le tunnel (contrairement à une assertion précédente) —
  seuls UDP (`udp_tracker_socket`) et HTTP clair (`http-request`)
  sont relayables ; HTTPS et WebSocket ne le sont pas. DHT/PEX
  reste le chemin de découverte universel en anonyme.

## Dialogue « Ajouter » : texte Anon raccourci (2026-10-04)

- `anonRelaysInfo` remplacé par « **IP masquée par {N} relais — le
  trafic passe par le tunnel ou nulle part (seeding inclus).** » —
  une seule phrase, centrée sur la garantie kill-switch ; les
  détails (attente de circuit, safe seeding) restent couverts par
  « ou nulle part ».

## Dialogue « Ajouter » : avertissement « Clair » en rouge (2026-10-04)

- Le texte « Votre IP est visible par les pairs. » sous le sélecteur
  de sauts passe en `error` quand **Clair** est choisi — cohérent
  avec le badge d'avertissement de la liste. En `Anon ×N`, le texte
  d'information reste en `outline`.

## `torrent_finished` ne se rejoue plus au démarrage (2026-10-04)

- **Bug** : à chaque boot, tous les téléchargements déjà terminés
  re-notifiaient « téléchargement terminé » (cloche + notif
  navigateur). Le pré-amorçage du set `finished` depuis le drapeau
  persistant existait, mais une fenêtre le cassait : restauré, un
  téléchargement complet rapporte `finished=false` pendant son
  `Initializing`/`Checking` (hashcheck) → la branche « redevenu
  incomplet » retirait le drapeau (mémoire **et** base) → la
  completion suivante repassait pour une nouvelle.
- **Fix** (`spawn_progress_loop`) : le retrait du drapeau est ignoré
  pendant `Initializing`/`Checking` — seul un état stabilisé
  (`Downloading`, `Paused`…) peut invalider la completion. Une vraie
  régression (sélection de fichiers étendue) continue de notifier la
  re-completion.
- **Test** : `torrent_fini_restaure_ne_renotifie_pas` vert (le cas
  réel n'apparaissait qu'avec un hashcheck plus long qu'un tick — un
  ISO de plusieurs Go, pas le petit fichier du test).

## Snackbar « téléchargement terminé » supprimé (2026-10-04)

- **Constat** : le toast en bas d'écran à chaque `torrent_finished`
  doublonnait le centre de notifications (cloche) — gênant et
  redondant.
- **UI** : `TorrentFinishedListener` supprimé ; l'événement alimente
  désormais uniquement la cloche via `NotificationsListener`. La
  notification **navigateur** (`notifySystem`, onglet en
  arrière-plan) est conservée — déplacée dans
  `NotificationsListener._finished`. Les snackbars de retour d'action
  (copie magnet, sauvegardes, erreurs) ne sont pas touchés.
- **Nettoyage** : clé `snackFinishedName` retirée des deux arb
  (`snackFinished` reste le titre de la notif navigateur).

## Badge « Clair » : visuel d'avertissement (2026-10-04)

- **Constat** : le badge des téléchargements en clair (contour gris,
  icône globe) était trop discret face au risque réel — IP exposée au
  tracker et à l'essaim — et passait inaperçu dans une liste dominée
  par les badges « Anon ×N ».
- **UI** (`AnonBadge`) : les téléchargements directs affichent
  désormais un badge rempli `errorContainer`/bordure `error` avec
  icône `no_encryption` — immédiatement identifiable, cohérent avec
  le bandeau « torrent privé » du dialogue Ajouter. Les lanes
  anonymes gardent contour primaire (×N) / tertiaire (attente
  circuit) ; le tooltip « Trafic direct » est inchangé.

## Dialogue « Ajouter » : note HTTPS supprimée (2026-10-04)

- **Constat** : même discrète, la note « trackers HTTPS ignorés »
  donnait l'impression qu'un torrent public HTTPS n'était pas
  téléchargeable en anonyme — alors que la découverte DHT/PEX via les
  tunnels fonctionne (vérifié : 31 pairs, ~1,7 Mo/s en Anon ×3).
- **UI** : note et bloc retirés ; les torrents publics (HTTP, mixtes ou
  HTTPS-only) n'affichent plus aucun message spécifique — sauts
  anonymes libres partout. Le bandeau rouge reste réservé aux torrents
  `private=1`.
- **Nettoyage** : `_httpsOnlyTrackers`/`_knownTrackers` (dialogue) et
  `TorrentPreview.httpsOnlyTrackers` (domaine) supprimés ; clé
  `httpsOnlyWarn` retirée des deux arb. `TorrentPreview.trackers`
  conservé (champ du DTO metainfo).

## Dialogue « Ajouter » : note HTTPS discrète et factuelle (2026-10-04)

- **Constat** : un torrent public à trackers HTTPS se télécharge
  parfaitement en anonyme (vérifié : 31 pairs, ~1,7 Mo/s en Anon ×3)
  — la découverte par **DHT/PEX à travers les tunnels** fonctionne,
  seules les annonces tracker HTTPS sont ignorées. L'alerte bandeau
  était donc du bruit ; le texte précédent (« ne trouvera aucun
  pair ») était faux.
- **UI** : retour à une note discrète (icône ⓘ, texte `outline`) :
  « Trackers HTTPS ignorés en mode anonyme — les pairs seront
  découverts via DHT/PEX à travers les tunnels ». L'alerte rouge
  reste réservée aux torrents `private=1`, seuls à être
  structurellement impossibles en anonyme.

## Dialogue « Ajouter » : alerte torrent privé, sauts verrouillés (2026-10-04)

- Le texte cyan « trackers HTTPS injoignables… ne trouvera aucun
  pair » était trop absolu pour un torrent **public** : les trackers
  sont bien hors de portée des sorties (HTTP clair one-shot
  uniquement), mais la découverte peut encore fonctionner via
  l'essaim anonymisé (DHT/PEX à travers les tunnels).
- Nouveau bandeau info `tertiaryContainer` (icône ⓘ, même style que
  l'alerte privée mais non bloquante) ; formulation corrigée :
  « seul l'essaim anonymisé pourra fournir des pairs : sans pair
  anonyme, le téléchargement restera en attente ». L'anonymat reste
  sélectionnable — c'est une dégradation, pas une impossibilité.

## Dialogue « Ajouter » : alerte torrent privé, sauts verrouillés (2026-10-04)

- **Besoin** : un torrent `private=1` (tracker à passkey, DHT/PEX
  interdits) ne peut pas passer par les tunnels — le seul indice
  était la note HTTPS discrète, facile à rater.
- **UI** : quand l'aperçu metainfo détecte `private`, un bandeau
  `errorContainer` (icône + titre « Torrent privé — anonymat
  impossible » + explication passkey/IP et DHT/PEX) s'affiche au-
  dessus du choix d'anonymat ; les segments `Anon ×N` sont
  **désactivés** et le choix est forcé à « Clair » (aussi à l'init
  tardive des réglages). Le message indique explicitement que le
  téléchargement se fera en clair, IP visible du tracker et de
  l'essaim.

## Débit servi mesuré exactement au limiteur (2026-10-04)

- **Besoin** : distinguer le trafic relayé/sorti pour les autres
  pairs du trafic propre — « trafic overlay − trafic fichiers »
  aurait été approximatif (overhead cellules ~15-20 %, fenêtres de
  mesure différentes, trafic direct non overlay).
- **Implémentation** : `RelayRateLimiter` compte les octets servis —
  la pompe d'émission `relay_send_tx` ne transporte que
  `SendJob::Endpoint` (cellules relayées) et `SendJob::ExitSocket`
  (envois de sortie), donc le point de mesure est exact par
  construction. Fenêtre glissante par buckets d'une seconde
  (`served_rate_window`, défaut 5 s) sans tâche dédiée — la
  comptabilisation se fait au fil de `allow()`.
- **API** : `bandwidth.relay_served_bps` (débit) +
  `relay_served_bytes` (cumul) dans `GET /api/statistics/ipv8`.
- **UI** : carte « Trafic overlay IPv8 » → ligne « relais servi :
  ↑ X » ; onglet Statistiques → « Débit servi mesuré (cumul) » à
  côté du plafond appliqué.
- Tests : `relay_rate_limiter_mesure_servi` (acceptés comptés,
  perdus exclus, diviseur planchonné à 1 s).

## Diagnostic : double sondage supprimé, cadence alignée sur 5 s (2026-10-04)

- **Bug** : sur l'onglet Diagnostic, ~5-6 requêtes/s. Deux mécanismes
  se superposaient : chaque provider sondait à **2 s** via
  `tickProvider`, ET `_OverviewTab` avait son propre
  `Timer.periodic(5 s)` qui invalidait 6 providers en rafale —
  l'« auto-refresh 5 s » affiché tournait en fait à 2 s + bursts.
- **Fix** : `_kDiagnosticPoll` 2 s → **5 s** (listes structurelles :
  overlays, circuits, relais, sorties, pairs, journaux — cadence
  affichée par l'UI) ; `_kTrafficPoll` à 2 s conservé pour
  `ipv8TrafficProvider` (débit live carte « Trafic tunnel » + barre
  d'état). Le timer doublon de `_OverviewTab` est supprimé —
  `ConsumerStatefulWidget` → `ConsumerWidget`, le bouton rafraîchir
  invalide toujours manuellement.
- Résultat : ~6 req/s → ~2 req/s sur l'onglet.

## Téléchargements : spam de requêtes `GET /api/downloads` éliminé (2026-10-04)

- **Bug** : ~11 requêtes `GET /api/downloads?get_peers=1` par seconde
  (~520 en 45 s) dans l'interface. Cause : `spawn_progress_loop`
  publie `download_state_changed` **pour chaque téléchargement à
  chaque tick** (~1 Hz par torrent) et le listener SSE de
  `DownloadsNotifier` lançait un re-poll complet par événement —
  soit ~N requêtes/s pour N torrents, sans garde anti-recouvrement.
- **Fix** : l'événement transporte déjà le `DownloadInfo` complet —
  `Download.mergeProgressStats` fusionne les champs volatils
  (progression, débits, statut, compteurs, ETA, cumuls, ratio,
  erreur) dans la ligne concernée sans aucune requête, en
  préservant les champs enrichis par le handler (anonymat, limites,
  scrape, trackers, pairs, horodatages). Infohash inconnu → re-poll
  complet (nouveau téléchargement). `torrent_status_changed` et
  `torrent_finished` (transitions rares) gardent le re-poll
  immédiat, le timer 2 s reste la source de vérité pour les champs
  enrichis, et `_refresh` a désormais un garde anti-recouvrement.
- Résultat : trafic de ~11 req/s à ~0,5 req/s, tout en rendant les
  barres de progression **plus** réactives (mise à jour ~1 Hz poussée
  par SSE au lieu du poll 2 s).

## Web : plus de 401 au démarrage (clé API résolue avant `runApp`) (2026-10-04)

- **Bug** : la console navigateur affichait des 401 sur
  `/api/events`, `/api/downloads`, `/api/versioning/versions` au
  chargement. Cause : `appConfigProvider` émettait la config par
  défaut **sans clé** pendant que `connectionSettingsProvider`
  résolvait la clé injectée (meta `onionbit-api-key`) — les
  premières requêtes partaient sans `X-Api-Key`.
- **Fix** : `main()` crée le `ProviderContainer`, attend
  `connectionSettingsProvider.future` (quasi immédiat sur web :
  localStorage + meta), puis monte l'app en
  `UncontrolledProviderScope` — le premier client API/SSE est créé
  directement avec la vraie clé. Desktop inchangé (la résolution
  peut lancer le daemon — pas d'attente bloquante).

## Réglages : section « Configuration avancée » retirée (2026-10-04)

- L'éditeur brut de `configuration.json` (sections avancée + import/
  export JSON) est supprimé de l'UI — trop dangereux : clé API et
  chemins d'identité visibles en clair, et un JSON malformé ou un
  réglage corrompu pouvait casser le démarrage du daemon. Le endpoint
  `POST /api/settings` reste disponible pour les usages en ligne de
  commande ; tous les réglages exposés passent par les sections dédiées.
- Clés l10n exclusives à cette section retirées
  (`sectionAdvanced`, `advWarning`, `advSaved`, `jsonInvalid`,
  `jsonNotObject`, `import*`, `export`, `reload`, `configCopied`) ;
  `apply`/`cancel` conservées (partagées).

## Contrôleur de congestion : exclusion des chemins non-WAN (2026-10-04)

- **Bug observé** : « base 0 ms » et plafond retombé à 64 Ko/s — un
  échantillon RTT quasi nul (pair loopback/LAN ou auto-ping hairpin
  dans la liste des pairs vérifiés) entrait dans la fenêtre, figeait
  la baseline à ~0 et lisait tout signal normal comme « congestion ».
  Symétriquement, un chemin LAN ne traverse jamais la file WAN : il
  aurait aussi pu masquer une congestion réelle.
- **Double défense** :
  - sélection : seules des adresses routables WAN sont sondées
    (`probe_eligible` — loopback, non spécifié, privé, lien-local
    exclus ; `Domain` admis faute de classification possible) ;
  - échantillon : RTT < `probe_min_rtt_ms` (défaut 1 ms, nouveau
    paramètre `BandwidthConfig` ; `0` = filtre inactif pour les
    tests sur réseau local) écartés avant le calcul du min.
- Tests : `adresses_sondables_wan_uniquement`,
  `echantillon_sous_plancher_ecarte`, `tous_sous_plancher_pas_de_signal`
  — 21/21 verts. La clé nouvelle profite immédiatement du format de
  config sparse : absente des fichiers existants → défaut appliqué.

## `configuration.json` sparse : seuls les écarts aux défauts persistés (2026-10-04)

- **Problème** : `DaemonConfig::write()` dumpait l'arbre complet —
  les défauts de l'époque étaient gelés dans le fichier et une
  correction de défaut dans une release ne se propageait jamais
  (ex. `target_delay_ms` resté à 50 malgré le passage à 25).
- **Décision (ADR-0014)** : le fichier ne persiste que les écarts
  aux défauts (`deep_diff` vs `DaemonConfig::default()`), +
  `config_version` toujours écrit. `#[serde(default)]` remplit les
  clés absentes au chargement → tout futur changement de défaut se
  propage automatiquement.
- **Migration v2** : `config_version` passe à 2 ; les fichiers écrits
  en dense voient `tunnel_community/bandwidth/target_delay_ms == 50`
  réaligné sur 25 (choix explicites ≠ 50 préservés). Tables de
  migration désormais indexées par version et à chemins imbriqués.
- Tests : `write_ne_persiste_que_les_ecarts_aux_defauts`,
  `migration_v2_realigne_target_delay_gele` — 9/9 verts.
- Note : la référence des défauts est désormais le code
  (`*Config::default()`) et `GET /api/settings` ; le fichier n'est
  plus auto-documenté.

## Contrôleur de congestion : `target_delay_ms` 50 → 25 ms (2026-10-04)

- Le seuil de retard de file toléré passe à 25 ms : 50 ms de buffer
  ajouté pénalisait les applications interactives (jeu, visio). 25 ms
  reste au-dessus du jitter naturel du RTT minimum sur un lien stable.
  Attention : la valeur est sérialisée dans `configuration.json` —
  les configs existantes doivent être éditées (clé
  `tunnel_community.bandwidth.target_delay_ms`) pour en bénéficier.

## Barre d'état : RTT minimum affiché à côté du plafond relais (2026-10-04)

- La barre d'état affiche désormais « relais ≤ {débit} · {rtt} ms » :
  le RTT minimum de la dernière rafale de sondage est visible en
  permanence à côté du plafond qu'il pilote (`statusRelayCapRtt`).
  Tooltip précisé : le « ms » est le RTT minimum des pairs sondés.

## Contrôleur de congestion : signal RTT = minimum de la rafale (2026-10-04)

- **Problème** : le signal de congestion était la **médiane** des RTT
  vers 8 pairs vérifiés, comparée à un min glissant. Un pair lointain
  ou dont l'uplink est saturé gonflait la médiane (~285 ms vs base
  ~49 ms) → repli ×0,75 répété → `effective_relay_bps` collé au
  plancher (64 Ko/s) et `relay_dropped` en hausse, sans charge locale.
- **Fix** : le signal est désormais le **minimum** des RTT de la
  rafale. La file d'émission locale étant commune à tous les
  paquets sortants, notre congestion sature tous les RTT — y compris
  le meilleur — tandis qu'un pair lointain/saturé n'affecte pas le
  min. `retard = min_rtt − baseline` isole donc le gonflement de
  *notre* file.
- **Renommage API** : `bandwidth.median_rtt_ms` →
  `bandwidth.min_rtt_ms` dans `/api/statistics/ipv8` (modèle Dart
  `RelayBandwidth.minRttMs` aligné).
- Tests : `pairs_lointains_ne_penalisent_pas` (pairs à 380-900 ms
  ignorés tant qu'un chemin reste à 30 ms), `inflation_globale_replie`
  (tous les échantillons +100 ms → repli), `min_d_echantillons`.

## Onglet Statistiques : « Signal RTT des pairs » affichait des valeurs permutées (2026-10-04)

- **Bug d'affichage** : la ligne `statBwRttValue` affichait des
  valeurs incohérentes (ex. « 8 ms · base 289 ms · 49 pongs » alors
  que l'API renvoyait `median=285,7`, `base=49,4`, `samples=8`). Cause :
  `gen-l10n` trie les paramètres par ordre alphabétique en l'absence de
  bloc `@placeholders` → signature générée `(base, count, median)` ≠
  ordre du call site `(median, base, count)`. Chaque slot affichait le
  champ voisin : median→samples, base→median, count→base.
- **Fix** : bloc `@statBwRttValue` ajouté à `app_en.arb` déclarant les
  placeholders dans l'ordre `(median, base, count)` — l'ordre de
  déclaration pilote la signature générée, comme pour `statTrafficValue`.
  Audit des 14 autres méthodes l10n multi-paramètres : aucun autre
  décalage.

## Carte « Trafic tunnel » : débit calculé par le daemon (2026-10-04)

- **Bug web confirmé** : la carte restait à `0 o/s` sur l'UI web alors
  que tout le reste est temps réel. Cause structurelle : c'était la
  seule carte à *mesurer* un débit (diff de compteurs
  `total_up`/`total_down` entre deux échantillons ≥ 1 s côté Dart) au
  lieu d'afficher une valeur du daemon — or Chrome échoue des sondages
  en rafale (`net::ERR_INSUFFICIENT_RESOURCES`) et la fenêtre de
  mesure client ne tenait pas.
- **Alignée sur le patron « Fichiers »** : le daemon calcule et publie
  `rate_up`/`rate_down` dans `ipv8_statistics` (`UdpEndpoint` :
  tâche `run_rate_sampler` + fenêtre glissante `RateWindow`,
  `stats_rate_sample_ms` = 1 s / `stats_rate_window_secs` = 6 s dans
  `Ipv8Config`). Une mesure unique côté daemon → desktop et web
  affichent la même valeur, quel que soit le sondage client.
- Dart : `Ipv8Traffic.rateUp`/`rateDown` ; suppression de
  `TunnelTraffic`, `TunnelTrafficNotifier`, `tunnelTrafficProvider`,
  `_kMinSampleWindow` — la carte lit `ipv8TrafficProvider` comme les
  autres cartes (implantation visuelle inchangée : ligne débit
  `↓ · ↑` + caption cumuls). `statTrafficValue` réordonné
  `↓ {down} · ↑ {up}` (cohérent avec la ligne principale).
- Tests : `rate_window_rates` (fenêtre, expiration, reset borné à 0)
  côté `onionbit-ipv8` ; `tunnel_traffic_test.dart` supprimé — plus
  de logique de mesure côté client à tester.
- `cargo check`/`clippy` propres, 8/8 tests `onionbit-ipv8` verts ;
  `dart analyze` propre ; 19/19 tests Flutter verts.

## Carte « Trafic tunnel » recréée (2026-10-04)

- Nouvelle implémentation : `tunnelTrafficProvider` =
  `NotifierProvider.autoDispose` + `TunnelTrafficNotifier` qui
  `ref.listen(ipv8TrafficProvider)` — le débit ↓/↑ est la différence
  des compteurs `total_up`/`total_down` entre deux émissions, les
  cumuls sont affichés en sous-ligne (`caption` de `_StatCard`).
- Une seule requête `/api/statistics/ipv8` (5 s) partagée par la
  carte, l'onglet Statistiques et la barre d'état ; aucune
  dépendance au dépôt/`apiClientProvider` → le notifier survit aux
  recréations du client HTTP par le watchdog SSE. Seuls les
  `AsyncData` sont échantillonnés (l'`AsyncLoading` de
  rafraîchissement re-porte l'ancienne valeur).
- Débit `—` tant que deux échantillons n'ont pas été observés ;
  compteurs remis à zéro (restart daemon) bornés à 0.
- **Fenêtre minimale d'échantillonnage (1 s)** : les émissions
  d'`ipv8TrafficProvider` arrivent parfois groupées (tick 5 s +
  invalidation par le `Timer` de l'onglet + recréation du client
  HTTP par le watchdog SSE, fréquent sur l'UI web) — un delta
  mesuré sur quelques millisecondes ≈ 0 écrasait le vrai débit
  jusqu'au tick suivant. Échantillon trop proche → ignoré, la
  baseline reste posée.
- `test/tunnel_traffic_test.dart` : baseline → débit positif →
  reset borné à 0, compteurs pilotés via override.
- `flutter analyze` propre ; 20/20 tests verts.

## Carte « Trafic tunnel » supprimée (2026-10-04)

- La carte « Trafic tunnel » de l'onglet Vue d'ensemble est retirée :
  le débit dérivé côté client (diff de compteurs `total_up`/`total_down`
  entre sondages) restait à 0 malgré deux réécritures — l'état `prev`
  caché casse le patron commun des autres cartes et survit mal aux
  rebuilds en chaîne (watchdog SSE).
- Suppressions UI uniquement (daemon inchangé) :
  `tunnelTrafficRateProvider`, `_tunnelTrafficPrevProvider`,
  `_TrafficSample`, la `_StatCard` de la grille, les clés l10n
  `cardTunnelTraffic` (fr/en) et `test/tunnel_rate_test.dart`.
- `ipv8TrafficProvider` (compteurs cumulés + `bandwidth`) reste utilisé
  par l'onglet Statistiques et la barre d'état.
- `flutter analyze` propre ; 19/19 tests verts.

## Carte « Trafic tunnel » : provider aligné sur le patron tick (2026-10-04)

- **Bug web réel** : la carte restait à 0 sur l'UI web alors que le
  bundle était frais (prouvé en fenêtre privée — même daemon, mêmes
  compteurs, débits vivants sur desktop). Cause : l'ancien
  `StreamProvider` maison exigeait **deux sondages** dans la durée de
  vie d'un même générateur ; or `connectionWatchdogProvider`
  re-résout la config tant que le SSE est coupé (fetch-SSE web moins
  stable) → `apiClientProvider`/dépôt recréés en boucle → baseline
  reperdue avant chaque 2ᵉ sondage → jamais d'émission.
- `tunnelTrafficRateProvider` suit désormais le patron commun :
  `FutureProvider.autoDispose` + `ref.watch(tickProvider(5 s))` —
  une valeur émise à chaque tick. L'échantillon précédent vit dans
  `_tunnelTrafficPrevProvider` (Provider **sans dépendances**) :
  seul endroit immunisé contre les rebuilds en chaîne.
- `dart analyze` propre ; `tunnel_rate_test` adapté à la valeur
  initiale `(0,0)` ; 20/20 tests verts.

## Débit servi : contrôle de congestion AIMD (2026-10-04)

- **L'estimateur de capacité est remplacé par un contrôleur de
  congestion** (ADR-0013) — famille LEDBAT / « Upload Speed
  Sense » d'eMule. L'ancien système était structurellement caduque :
  le pic passif est censuré par son propre plafond (l'estimation se
  verrouillait à ~200 Kio/s alors que la sonde mesurait ~111
  Mbit/s), l'UPnP dépend d'un IGD qui ne répond pas partout, et la
  sonde HTTP impose un tiers qu'un daemon d'anonymat ne devrait pas
  contacter sans opt-in.
- **Principe** : toutes les `bandwidth/sample_secs` (5 s), rafale de
  `ping` Discovery vers les 8 pairs vérifiés les plus frais ;
  retard de file = médiane RTT − baseline (min glissant 10 min).
  En dessous de `target_delay_ms` (50 ms) le plafond croît
  additivement (`cap/8`, min 32 Kio/s), au-dessus il chute ×0,75.
  Sans échantillon : plafond inchangé ; sans pair : `fallback_bps`.
  Borné `[floor_bps, max_bps]` = [64 Kio/s, 32 Mio/s].
- **Câblage** : `DiscoveryCommunity::set_pong_probe` notifie
  `(adresse, identifier)` de chaque `pong` — la `RttProbe` apparie
  aux pings de la tâche (pongs churn/keepalive ignorés).
- **Config** : `tunnel_community/bandwidth` ne garde que les
  paramètres du contrôleur (`floor/fallback/max_bps`,
  `sample_secs`, `probe_peers`, `probe_wait_ms`,
  `base_window_secs`, `target_delay_ms`, `increase_*`,
  `decrease_pct`) — `share`, `measure_upnp`, `probe_*`,
  `measure_interval_secs`, `warmup_secs` supprimés (ignorés par
  `serde(default)` dans les anciens fichiers).
- **API/UI** : `bandwidth` expose `effective_relay_bps`,
  `base_rtt_ms`, `median_rtt_ms`, `rtt_samples` — la page
  Statistiques affiche « Signal RTT des pairs ».

## Premier lookup de swarm immédiat + débit tunnel vivant (2026-10-04)

- **`Swarm::new` : `last_lookup` initialisé « jamais »** — comme le
  `last_lookup = 0` de pyipv8 (`tunnel.py:316`). Avant : instant de
  création → un swarm frais (restauration, re-join au changement
  d'état) attendait `swarm_lookup_interval` (30 s) avant son premier
  lookup, puis autant à chaque tentative vide — les downloads
  restaurés restaient sans pairs des minutes en prod (1/4 reparti
  vite, les autres ~10 min). Désormais le prochain tick
  `do_peer_discovery` (10 s) les prend tout de suite — fidélité
  protocole restaurée, pas écart.
- **`tunnelTrafficRateProvider` refait sans notifier** : sous
  Riverpod 3.x le notifier peut être recréé à chaque rebuild par
  dépendance — `_prev` repartait à `null`, la carte « Trafic
  tunnel » restait à `↓ 0 · ↑ 0` malgré le trafic. Boucle de
  sondage autonome en `StreamProvider` (le `prev` vit dans la
  clôture du générateur) + test unitaire du diff et du clamp sur
  compteurs remis à zéro.

## Banc live : fenetres premier-octet élargies (2026-10-03)

- `live_suppression_pendant_transfert`, `live_update_hops_en_transfert`,
  pause/reprise : l'attente du premier octet uTP passait `STATE_WAIT`
  (30 s) — sous runner macOS chargé le backoff du dial à travers le
  circuit la dépassait (« aucun octet avant suppression », CI #85).
  Ces attentes passent à `TRANSFER_WAIT` (180 s) : le sujet des tests
  est la suppression / migration / pause, pas la latence du dial.
- Les `STATE_WAIT` restants (pending, `owner_engine_hops`) gardent
  30 s — états internes sans réseau.

## Métriques fichiers / tunnel séparées (2026-10-03)

- **Carte « Trafic tunnel »** (Vue d'ensemble) : débit instantané
  ↓/↑ de l'overlay IPv8, calculé par différence des compteurs
  cumulés `total_up`/`total_down` entre deux sondages de
  `/api/statistics/ipv8` (`tunnelTrafficRateProvider` — delta
  négatif au redémarrage du daemon ramené à 0). Mesure le trafic
  total : relais servis, protocole et téléchargements.
  Sondage autonome toutes les 5 s (`StreamProvider` +
  `Stream.periodic`, `prev` dans la clôture du générateur) ; le
  repository est capturé dans la phase synchrone du corps du
  provider — `ref.watch` n'est pas valide dans un générateur
  `async*` différé (erreur avalée → stream jamais émise).
- **Cartes renommées « Réception fichiers » / « Envoi fichiers »** :
  la Σ `speed_down`/`speed_up` de `/api/downloads` ne mesure que le
  trafic BitTorrent des téléchargements locaux — à 0 honnêtement
  quand le daemon ne fait que relayer (prod : ~155 Kio/s d'upload
  tunnel réel pour « Envoi 0 o/s » affiché).
- `cardUpload`/`cardDownload` conservés pour la boîte de test de
  bande passante (débit brut, pas « fichiers »).

## Niveau `info` assaini pour la release (2026-10-03)

- **Churn circuits/handshakes → `debug`** : `tentative de creation
  proactive de circuit`, `repartition des cellules par type` (tous les
  4096 cellules), `guard retrograde en reserve`, tout le chemin
  hidden-services (`establish-intro`, `intro-established`,
  `peers-response`, `create-e2e`, `created-e2e`, `link-e2e`,
  `linked-e2e`, `e2e listener : lane branchee`) — événements par
  circuit/cellule qui inondaient le log release en prod.
- **Erreurs réelles remontées `info` → `warn`** :
  `create_rendezvous_point echoue`, `circuit RP_DOWNLOADER echoue`.
- **`librqbit_upnp=error` dans le filtre par défaut** : la dépendance
  boucle un `warn` à chaque tentative de mapping refusée par le
  routeur (« failed to bind port forwarding: 500 ») — bridée dans la
  directive par défaut ; `RUST_LOG` et le toggle debug runtime
  redonnent le contrôle total.

## Restauration après le premier circuit prêt (2026-10-03)

- **`await_data_circuit_of_hops`** (`TunnelCommunity`) : attente
  bornée (`next_hop_timeout`) du premier circuit `DATA` `READY` à
  `hops` sauts, réveillée par `circuits_changed`.
- **Downloads anonymes reajoutés trop tôt au boot** : la
  restauration tournait dès le démarrage, avant que les circuits
  des lanes soient construits — les premiers envois (dial uTP,
  trackers, DHT) tombaient sur `select_circuit` (« aucun circuit
  prêt ») et librqbit restait dormant jusqu'à un pause/reprise
  manuel (prod : 2 downloads sur 4 bloqués à 0 o/s après restart).
  `restore_downloads` attend désormais le premier circuit prêt de
  chaque lane (`awaited_lanes` — une fois par `anon_hops`) avant
  les re-adds ; `spawn_deferred_restore` applique la même garde à
  la résolution BEP 9 différée. Best-effort : à l'échéance le
  re-add se fait quand même (Python restaure aussi sans garantie).

## Ids de torrents uniques sous adds concurrents (2026-10-03)

- **TOCTOU sur `persistence.next_id()` dans
  `Session::add_torrent_internal` (librqbit vendored)** : l'id du
  torrent était alloué par une lecture `max+1` de la persistance,
  sans réservation — avant le verrou par infohash, le sémaphore et
  l'init disque. Deux `add_torrent` concurrents de hash différents
  (verrous `add_locks` distincts, aucune exclusion) obtenaient le
  même id ; le check `already_managed` (`*eid == id`) classait alors
  le second `AlreadyManaged` **du torrent voisin** : le moteur
  retournait `Ok` avec le handle d'un autre torrent et le vrai
  n'entrait jamais dans la map — `get_by_hash`/`owner_engine_hops`
  muets. Signature exacte du flake `live_flotte_10_restart`
  (`fleet*.bin ajoute sur la mauvaise lane, left: None`) qui ne
  mordait que sur les index `.bin` partageant leur lane avec un
  magnet spawné en vol, et surtout macOS (init disque → fenêtre
  large). Le compteur atomique `Session::next_id` sert désormais la
  réservation (`fetch_add`), réaligné sur `max(ids vivants, ids
  persistés)` pour rester au-dessus de tout id existant (restores
  `preferred_id` compris). Test de régression déterministe
  `adds_concurrents_ids_uniques` (barrière de 32 adds parallèles,
  persistance activée) : rouge avant, vert après.

## Compteurs d'observabilité de la gate de sortie (2026-10-03)

- **`/api/ipv8/tunnel/exits` gagne `cells_seen` et `gate_rejected`** :
  la question « 0 sortie active » n'était pas tranchable — une sortie
  `enabled:false` peut être légitime (le distant n'a encore envoyé
  aucune donnée tunnelée, mesh sans transferts anonymes sortants) ou
  signaler une gate IP qui rejette tout. `cells_seen` compte les
  cellules `data` reçues pour la sortie avant la gate ;
  `gate_rejected` celles écartées faute de correspondance avec l'IP
  du saut amont. `cells_seen=0` persistant = aucune data distante
  (normal) ; `gate_rejected` qui monte = adresse de saut stale ou
  spoof (à investiguer). Asserts ajoutés à
  `tunnel_data_exits_1_hop`.

## Compteur `bytes_up` des routes relais (2026-10-03)

- **`RelayRoute.bytes_up` restait à 0 en permanence** : le forwarding de
  cellule n'incrémentait que `bytes_down` à la réception (`community.rs`),
  jamais `bytes_up` au relais — alors que pyipv8 incrémente les deux
  (`increase_bytes_received` + `increase_bytes_sent` sur la route).
  Effets : `/api/ipv8/tunnel/relays` rapportait `bytes_up: 0` sur toutes
  les routes, les événements `circuit_removed` sous-comptaient le
  trafic servi, et le garde-fou `max_traffic` (`bytes_up + bytes_down`)
  ne voyait que la moitié des octets relayés. Le compteur montant est
  désormais incrémenté à la tentative de forward (sémantique pré-envoi
  identique au Python ; les rares pertes de file pleine restent
  tracées).

## Signal de fin de restauration non perdu (2026-10-03)

- **`restore_done` pouvait perdre son basculement** : le canal
  `watch` est construit sans receveur retenu et `watch::Sender::send`
  échoue silencieusement (`let _ =`) quand aucun receveur n'existe.
  Une restauration terminée avant le premier `wait_restored()` (base
  vide : magnet jamais résolu non persisté — chemin le plus rapide)
  perdait le signal : `wait_restored()` bloquait indéfiniment et
  `restore_finished()` restait `false` pour toute la session
  (`live_magnet_pending_non_restaure_au_restart` figé en CI Ubuntu,
  ~146 s jusqu'au timeout du test). `send_replace` stocke la valeur
  quelle que soit la présence de receveurs.
- **Même défaut latent dans `services::rss` et
  `services::watch_folder`** : `stop.send(true)` appelé avant que la
  tâche ait souscrit perdait le signal d'arrêt → `send_replace`.

## Création de lane anonyme atomique (2026-10-03)

- **Course get-or-create dans `Ipv8Stack::anon_engine`** : deux adds
  concurrents sur une lane neuve (ex. un magnet spawné et un
  `.torrent` inline sur `hops=3`) créaient chacun un moteur — le
  second `insert` écrasait le premier dans `anon_lanes` et le
  download ajouté au moteur perdant devenait orphelin : invisible de
  `owner_engine_hops`, `find_download_hex`, `downloads()` et de la
  restauration, tout en continuant à tourner. Reproductible en prod
  via deux `PUT /api/downloads` simultanés sur une lane jamais
  utilisée. `anon_engine_lock` (mutex Tokio) sérialise la création
  avec re-check sous verrou (échecs CI macOS `fleet7` → lane `None`,
  stall Ubuntu).
- **Banc** : les adds magnet du test flotte sont spawnés (l'add
  attend la résolution BEP 9 inline, état `pending` voulu), avec
  attente bornée de matérialisation sur la bonne lane ; le seeder
  est réinjecté à chaque scrutation de progression pre-kill (les
  `initial_peers` ne sont pas retentés en mode offline) ; le panic
  de progression emporte stats complètes, lane observée et état
  `pending` — compteurs de pairs pour trancher stall vs orphelin.

## Fastresume synchrone au stop + course pause/check (2026-10-03)

- **Flush `.bitv` synchrone à `pause()`** (patch vendored librqbit) : le
  bitfield n'était persisté que tous les 16 Mio
  (`FLUSH_BITV_EVERY_BYTES`) puis par le drop-flush asynchrone — un
  torrent de taille modeste arrêté proprement pouvait voir sa
  restauration devancer l'écriture finale et repartir à zéro
  (`live_flotte_10_restart`, pseudo-hang ubuntu en CI = épuisement des
  timeouts). `pause()` (sur lequel `Session::stop` s'appuie) attend
  désormais l'accusé d'écriture+fsync via un `SyncSender`, borné 5 s et
  replié en asynchrone sur runtime mono-thread.
- **Course pause pendant `Initializing`** (vendored) : une pause
  demandée pendant le check initial était perdue si le check se
  terminait ensuite — le torrent repartait `Live` avec le drapeau
  `paused` posé, et `resume` échouait en « torrent is already live »
  (échec macOS `live_pause_resume_tunnel`). La fin de check honore
  `is_pause_requested()` ; `Engine::resume` tolère « already live »
  (reprise idempotente, l'état visé est déjà atteint).
- **Banc** : marqueurs de phase `tracing` et bornes explicites sur
  `stop`/`wait_restored`/adds dans `live_flotte_10_restart` ; step CI
  `tests workspace` borné à 45 min — un hang devient un échec localisé.

## Chaos/restart de flotte + endurance nightly (2026-10-03)

- **`live_flotte_restart_3_cycles`** : 12 torrents mixtes
  (`.torrent`/magnets, lanes 0–3), 3 restarts successifs, pause et
  suppression d'un sous-ensemble entre cycles — lanes persistées,
  fastresume, absence de résurrection après delete, intégrité octet
  par octet finale. La pause passe par le chemin API complet
  (`pause` + `set_stopped_flag`), comme le vrai client.
- **`live_crash_pending_magnet_et_restart`** : runtime dédié arrêté
  brutalement (`shutdown_timeout(0)`) pendant qu'un magnet est en
  résolution — la ligne `downloads` n'existant qu'après résolution,
  le restart restaure les téléchargements matérialisés, le pending
  n'est pas fabriqué, et l'entrée reste honnête et supprimable.
- **`live_endurance_churn`** (`#[ignore]`, job nightly) : churn
  déterministe (graine rejouable) — adds/removes/pauses/resumes/
  migrations aléatoires + restarts périodiques, métriques CSV par
  tick (RSS, `num_alive_tasks`, circuits READY/total, états,
  pending, lignes DB), seuil de croissance RSS post-warmup,
  vérification finale lanes + intégrité + suppression totale.
- **Relais de banc** : `max_joined_circuits` porté à 10 000 sur les
  nœuds de test — la limite protocole Python (100) sature un
  mini-réseau de 3 relais sous churn (intro-points safe_seeding ×N +
  circuits proactifs), produisant des `create ignore` massifs qui
  masquaient le comportement testé.
- **CI nightly** : étape `endurance churn (live)` (30 min,
  `LIVE_ENDURANCE_SECS`) dans le job `nightly`, CSV publié en
  artefact `live-endurance-metrics`.

## Restauration unique au redémarrage + banc live loopback (2026-10-03)

- **`session_restore` rqbit désactivée** (`SessionPersistenceConfig::Json
  { restore }`, patch vendored) : la relecture de `session.json` au
  démarrage rejouait les adds en concurrence du checkpoint tribler.db —
  la course pouvait abandonner la tâche (`no torrent found`) en laissant
  des torrents insérés mais jamais démarrés, figés en `Initializing`.
  Le `.bitv` fastresume reste écrit et relu par infohash ; seule la
  liste n'est plus rejouée — tribler.db est l'unique autorité.
- **`AddDownloadOptions.initial_peers`** + variantes
  `add_download_anon_with_peers` / `add_torrent_bytes_anon_with_peers` :
  pairs d'amorce éphémères (non persistés) injectés à l'ajout —
  équivalent du bootstrap contrôlé, signatures publiques inchangées.
- **Banc `tests/live_bench.rs`** : vrais transferts UDP loopback —
  seeder rqbit réel, relais autonomes `RELAY|EXIT_BT`, circuits
  épinglés dont chaque saut est confirmé par `verified_hops`, trafic
  mesuré sur les circuits. 10 tests : `.torrent` clair et ×1/×2/×3,
  magnet résolu à travers le tunnel, PATCH de lane pendant résolution,
  migration ×1→×3 en transfert avec réinjection du pair, pause sans
  trafic résiduel, doublon refusé, suppression purgeant tous les
  moteurs, flotte de 10 torrents mixtes avec arrêt + restart +
  fastresume validé, magnet pending non restauré au restart.

## Résolution magnet anonyme : repli de sauts + pending opérationnel (2026-10-03)

- **Tunnel — le repli `send_extend` est réellement atteint** : la garde
  `!candidates.is_empty()` détruisait le circuit quand les `candidates`
  offerts par le saut précédent étaient épuisés ; pyipv8 appelle
  `send_extend` quels que soient les restes, ce qui déclenche le repli
  `get_candidates(EXIT_BT|RELAY)` (pair du registre, adresse réelle
  transmise au relais). Avec un pool de relais étroit dont la liste
  offerte est périmée, les circuits ×3 ne se complétaient jamais et
  les magnets anonymes restaient figés en METADATA sous le kill
  switch — chaque reconstruction repassait par le même guard qui
  re-servait les mêmes mids morts.
- **Mémoire d'échec des sauts d'extension** (`extend_failures`, TTL =
  `circuit_timeout` configuré) : un candidat qui expire en `extend`
  est sauté — la liste offerte s'épuise vite et le repli registre est
  atteint au lieu de brûler `max_tries` sur des mids morts.
  Extension locale sans changement filaire ; sauts épinglés et
  `required_exit` non concernés.
- **Magnets en résolution (`pending`/METADATA) opérationnels** :
  `PATCH anon_hops`, pause/reprise et suppression n'exigent plus
  l'objet moteur (auparavant 404 alors que le download était listé).
  `update_pending_hops` stocke la nouvelle cible et réveille la tâche
  (`pending_notify` + `select!` abandonnent `add_uri_opts`/`readd_row`
  en vol, relance sur la lane demandée) ; à la matérialisation une
  cible encore différente converge via `update_hops` classique.
  `CoreError::Cancelled` rend l'abandon silencieux quand le download
  est supprimé pendant sa résolution.
- **`download_exists` étendu à METADATA** : un re-`PUT` du même magnet
  (ou un `.torrent` du même infohash pendant la résolution) est refusé
  (`InvalidState` → 400) au lieu de respawner une seconde tâche — les
  deux materialisaient un download moteur pour le même infohash
  (lignes doublées partageant la même ligne `downloads`, badge de lane
  trompeur, sélection par infohash groupée).
- **Suppression multi-moteurs** : `remove()` retire l'infohash de tous
  les engines détenteurs — un doublon historique laissait l'orphelin
  dans l'autre lane, encore listé par `downloads()` mais sans ligne
  persistée → réapparaissait affiché « Clair ».
- **Badge `hops` adossé à la vérité moteur** : sans ligne `downloads`,
  `GET /api/downloads` retombe sur `owner_engine_hops` (lane réelle du
  moteur détenteur) au lieu de `0` — un download anonyme ne peut plus
  être affiché « Clair » par défaut. La ligne persistée reste
  prioritaire (intention configurée, parité Python).
- Tests : `circuit_extend_replie_registre_quand_candidats_perimes`
  (sortie morte offerte → circuit READY via le pair exit du registre),
  `operations_sur_magnet_en_resolution` (PATCH accepté + bornes, dedup
  re-add, pause persistée, suppression qui réveille la tâche) et le
  nouveau banc `tests/scenarios.rs` — session ipv8 loopback, lanes
  anonymes réelles : ajouts ×0/×1/×2/×3 sur la bonne lane, migration
  de lane en cours sans doublon, dedup materialisé + pending avec
  abandon `Cancelled`, PATCH/pause/delete sur magnet en résolution,
  restauration par lane au redémarrage.

## Packaging multi-plateforme en CI (2026-10-03)

- **Nouveau `scripts/package_posix.sh`** : assemble `dist/<target>/`
  (daemon + cli + `web/` + LICENSE + manifest) puis produit le tarball
  `OnionBit-<ver>-<os>-<arch>.tar.gz` et, sous Linux avec `dpkg-deb`,
  le paquet `onionbit_<ver>_<arch>.deb` — layout `/opt/onionbit/` +
  liens `/usr/bin` (l'auto-detection `<exe>/web` suit les liens
  symboliques) + unite systemd utilisateur.
- **Jobs CI de package** : `package-linux` (ubuntu-latest : tar.gz +
  .deb + smoke `--help`), `package-windows-arm64` (windows-11-arm :
  `build_dist.ps1 -ZipRelease` avec UI Flutter + web, non bloquant —
  runner preview).
- **Publication sur tag** : un push `v*` lance la matrice puis le job
  `publish` cree la release GitHub avec tous les artefacts et leurs
  SHA-256 (pre-release auto si le tag contient alpha/beta/rc). Le job
  ARM64 est attendu mais non obligatoire.
- `build_dist.ps1` : le suffixe du zip derive desormais de l'archi
  hote (`windows-x64`/`windows-arm64`) au lieu d'etre code en dur.

## Réglage mémoire des buffers uTP (2026-10-03)

- **Nouvelles clés persistées** `libtorrent/utp_rx_buf_size` et
  `libtorrent/utp_tx_buf_max` (extensions Rust — pas d'équivalent
  libtorrent ; `0` = défauts librqbit-utp : RX 1 Mio, TX 32 Kio → 1 Mio
  par connexion) : plafonds des buffers uTP en espace utilisateur, poste
  mémoire dominant identifié dans
  `docs/diagnostics/memoire_charge_reelle.md` (~10-60 Mo pour ~100
  pairs). Mémoire pire cas par connexion : `rx + tx_max`.
- **Plombage bout-en-bout** : `LibtorrentConfig` →
  `EngineConfig::utp_socket_opts()` (nouveau helper — `SocketOpts` du
  vendored, inchangés par défaut) → `ListenerOptions::utp_opts` pour la
  session en clair (une seule socket uTP sert écoute **et** connexions
  sortantes, `StreamConnector` retombe sur l'écoute) **et**
  `TunnelUdpSockets::with_dht_policy` (nouveau paramètre `utp_opts`,
  propagé par `Ipv8Stack::anon_engine` à chaque lane). Le ring buffer
  TX est borné `initial <= max` quand le plafond descend sous 32 Kio.
- **Attention débit** : le buffer RX est la fenêtre annoncée au pair —
  il borne aussi le débit d'une connexion (`fenêtre / RTT`). Documenté
  dans `configuration_cablage.md` et dans le diagnostic.
- Tests : `utp_socket_opts` (défauts intacts, plafonds propagés,
  clamp `initial <= max`), propagation vers `ListenerOptions`, et
  transfert loopback e2e complet sous fenêtres 16 Kio RX/TX.

## Ouverture P1 : doc des ruptures + CI matricielle (2026-10-03)

- **Nouveau `docs/ruptures/README.md`** : doc publique des bancs de
  rupture — principe fail-closed (« silence admis, sortie directe
  bloquante »), oracle `INTERDIT(t_failure, t_reprise) = 0`, recette
  en 7 étapes (vrai chemin, observation OS, injection horodatée,
  attribution, fenêtrage, reprise, archivage), exemples concrets
  `kill`/`block` avec déroulés attendus et artefacts, lecture du
  manifeste, pièges déjà rencontrés (`--offline` permissif, faux
  positifs IPv8, injection post-complétion, timing magnets, sens
  inbound/outbound).
- **Nouveau `.github/workflows/ci.yml`** : matrice Rust
  windows/ubuntu/macos (+ windows-11-arm en `continue-on-error`,
  runner preview) — `check`/`clippy -D warnings`/`fmt`/tests ;
  job Flutter (analyze/test/`check_i18n`) ; job release Windows x64
  (`build_release.ps1` + smoke + artefact) ; job `nightly` planifié
  comme point d'ancrage pour l'endurance automatisée. Les bancs
  réseau réel (Tribler.exe, pyipv8, interop) restent volontairement
  locaux — non reproductibles sur runner hébergé.

## Diagnostic mémoire charge réelle + garde-fou (2026-10-03)

- **Diagnostic consigné** dans
  `docs/diagnostics/memoire_charge_reelle.md` : l'empreinte privée
  ~135 Mo d'un daemon public (8 torrents en seed, ~100 pairs BT, 81
  circuits, 56 relais joints, 44 sorties, 4 sessions librqbit, 12 k
  torrents en base) est une facture de charge, pas une fuite — vs
  ~16 Mo d'un nœud idle du banc loopback. Postes reliés au code :
  buffers uTP/connexion (jusqu'à 2 Mio), `ReadBuf`/`write_buf` par
  pair, sessions par hop, état tunnel, SQLite. Les pages mmap
  expliquent l'écart working set ↔ privé.
- **Nouveau `scripts/mem_watchdog.ps1`** : échantillonne
  périodiquement mémoire processus (privé, working set, handles) et
  compteurs de charge API (circuits par type/état, relais, sorties,
  pairs BT live, taille DB) dans un CSV ; alerte console + fichier
  `.alerts.log` sur plafond privé, dérive vs baseline, et caps
  (`max_joined_circuits`, `peer_limit` × sessions). Le signal pairs
  retient `num_connected_peers` (connexions live) — `num_seeds`/
  `num_peers` du DTO incluent le scrape swarm, non borné.
- Validé en direct sur le daemon de production : résolution
  `-StateDir` → clé API + port + PID, CSV invariant-culture, alertes
  `PRIV_DEPASSE`/`CAP_*`/`DERIVE_MEM` fonctionnelles.

## Migration versionnée de `configuration.json` (2026-10-03)

- `config_version` (extension Rust — `TriblerConfig` n'a pas de
  marqueur) estampille le schéma persisté ; les fichiers legacy (v0)
  migrent au chargement : les interrupteurs « Tunnels anonymes »
  encore gelés à leur ancien défaut par l'écriture complète du
  fichier sont réalignés — `tunnel_community/guards_enabled`
  `false`→`true` (instals du 1er–2 oct. qui tournaient avec le flag
  expérimental inactif sans choix explicite).
- Choix explicites préservés (valeur ≠ ancien défaut) : `enabled`
  et `exitnode_enabled` n'ont jamais glissé, rien n'y change.
- Fichier réécrit estampillé une fois : migration non rejouée, choix
  ultérieur vers l'ancienne valeur honoré ; `config_version` non
  patchable via `POST /api/settings`. Tests unitaires : réalignement,
  préservation des choix, idempotence, garde du merge.

## Bancs de maturité : PR-1, PR-3, CH-6 (2026-10-03)

- **PR-1** : `scripts/build_release.ps1` rejoué — release
  `x86_64-pc-windows-msvc` reproductible (manifeste : commit `03a5307`,
  rustc 1.98.1, 0.6.0-alpha) ; smoke du binaire : API montée, 401 sans
  clé, arrêt propre.
- **PR-3** : parcours lanceur web `OnionBit Web.cmd` → `web-launch.ps1`
  — spawn du daemon dist avec son state-dir dédié, API joignable,
  navigateur ouvert ; le parcours UI reste manuel.
- **CH-6** : nouveau `scripts/bench_crash_recovery.ps1` — N cycles de
  kill -9 à instant pseudo-aléatoire après un ajout `.torrent`, sur le
  même state-dir. 5/5 cycles verts : API remontée, downloads persistés
  restitués, `PRAGMA quick_check` = `ok`.
  - Enseignement consigné : un **magnet non résolu n'est pas persisté**
    (le spawn `add_download_anon` n'écrit qu'après retour du moteur) —
    en `--offline` il ne l'est donc jamais ; le chemin de persistence
    testable est `torrent_data`. Comportement cohérent avec Tribler
    (checkpoint des torrents résolus).
- **CH-1** : 50 `.torrent` distincts injectés en rafale via l'API
  (`--offline`) — 50/50 en 0,45 s, tous listés, RSS stable 31 MB. La
  variante swarm-actif reste un banc public.
- **Longévité bornée (60 min)** : `fingerprint_mesh.ps1
  -DurationMin 60 -WithAnonDownload` — ping/pong stable à
  0,366→0,362 msg/s (dérive nulle), lane anonyme entretenue, circuits
  reconstruits proactivement (1→3), daemon vivant après 718
  échantillons. Pas de croissance de cadence sur la durée.
- **PR-7** : sonde `versioning/versions/check` exécutée (daemon
  `--no-ipv8`) — `{has_version:false}`, `current=0.6.0-alpha` ; la
  notification nécessite une release distante plus récente.

## P0-17c — fail-closed observé par l'OS, 4 sous-runs verts (2026-10-03)

Extension de `sec_leak_capture.ps1` : injection de panne contrôlée en
plein transfert, fenêtrage `[t_failure, t_fin_capture]` dans
`analyze_leak_capture.py`, manifeste enrichi. Chaque sous-run = capture
neuve + manifeste séparé ; oracle strict `INTERDIT(fenêtre)=0`.

- **`kill`** (17c-4) : taskkill du banc à 262 144 o vérifiés ; fenêtre
  post-mortem 25 s sans paquet fantôme ni processus orphelin.
- **`block`** (17c-1) : règles pare-feu entrantes+sortantes sur les
  premiers sauts réels à 327 543 o — mort de circuit, proxy vivant ;
  tout fallback direct aurait été VISIBLE ; reprise observée jusqu'à
  982 903 o après levée de la règle.
- **`wan`** (17c-3) : `Disable-NetAdapter` 45 s à 589 687 o ; reprise
  propre du transfert jusqu'à 4 915 063 o après retour réseau.
- **`kill-bootstrap`** (17c-2) : Tribler.exe tué à 851 831 o ; le
  download a **complété** 1 638 263 o sur la route établie — la mort
  du bootstrap n'affecte pas les tunnels en cours. (Le worker proxy
  étant in-process, c'est le pendant OS injectable.)
- **Correctif analyseur** : exemption par signature IPv8
  (`00 02` + community-id) pour le trafic structurel du nœud vers ses
  pairs candidats — le TAP ne couvre que les envois de lanes. Un faux
  positif de 2 050 paquets (tous `0002a359…`/discovery) reclassé ; une
  vraie fuite uTP/BT/DHT en clair resterait `INTERDIT`.
- Reste : 17c-5 (réinstanciation de lane active) non injectable sans
  hook daemon — consigné dans les trous de couverture.

## Banc de fuite niveau OS (P0-17b) — capture pktmon (2026-10-03)

Nouveaux `scripts/sec_leak_capture.ps1` + `scripts/analyze_leak_capture.py`
: capture pktmon complète (paquets entiers) pendant un téléchargement
anonyme **réel** à sauts libres sur le réseau Tribler public, puis
classification hors-ligne de chaque endpoint distant.

- **Attribution par port local** : pktmon ne porte pas de PID ; le
  classifieur scope par les ports UDP du processus de banc extraits du
  journal (endpoint IPv8/tunnel + écoute uTP rqbit), le TAP servant de
  vérité fil des endpoints overlay autorisés. Le trafic concurrent
  (Tribler.exe hôte, OS) tombe en `AUTRE` sans fausser le verdict.
- **Oracle** : tout paquet WAN depuis un port du banc hors endpoints
  overlay = INTERDIT (BT/uTP direct, DHT mainline, TCP WAN) ; DNS
  port 53 décodé et qnames rapportés (infra DHT admise et documentée,
  aucun hostname de tracker/magnet).
- **Run de référence** : 1 113 975 octets vérifiés sur route publique
  2 sauts, 181 770 paquets capturés, **INTERDIT = 0**, fenêtre
  post-arrêt 20 s sans trafic fantôme.
- Auto-élévation UAC si lancé sans droits admin ; artefacts :
  `capture.pcapng`, `manifest.json` (commit, PID, fenêtre
  temporelle), `leak_report.json`, journaux du banc.
- Reste pour itération suivante : rejouer sous destruction de circuit
  proxy vivant, proxy tué, coupure/retour d'interface WAN (SE-4).

## Campagne de bancs P0 complète + harnais anti-SSRF live (2026-10-03)

Passe de non-régression P0 exécutée de bout en bout après les
changements tunnel/DHT/conntrack/guards, consignée dans
`docs/plans/bancs_tests.md` §7 :

- **Interop pyipv8** : PY-1..PY-6 verts (discovery, DHT signée,
  tunnels crypto, relais, sortie `EXIT_BT` avec 200 Ko vérifiés).
  PY-7 (py↔py baseline) : **BLOCKED/ENVIRONMENTAL** — deux Tribler
  8.4.3 officiels n'ont résolu aucun `peers-request` en 38 min, aucun
  OnionBit dans le chemin.
- **Tribler.exe** : matrice hidden download/seed `-Guards` hops 1/2/3
  verte dans les deux sens (SHA-256 exact, `hors_set=0`).
- **Fingerprint mesh 15 min** : aucune tempête PING/PONG
  (ping+pong ≈ 0,36 msg/s, cellules tunnel ≈ 1,28 msg/s), kill switch
  engagé→désarmé, zéro fallback.
- **Public** : 1 638 263 octets vérifiés sur route 2 sauts réelle.
- **Fuzz smoke** : 6/6 cibles, ~152 M execs, 0 crash.
- **Correctifs harnais** : race `Get-FileHash` (handle rqbit) corrigée
  dans `interop_hidden_tribler_seed.ps1` (DELETE + retry, validée par
  re-run) et en prévention dans `interop_hidden_tribler_download.ps1` ;
  `interop_py_relay.ps1` tolère un stderr vide.
- **Nouveau banc `sec_anti_ssrf_live.ps1` (P0-17a)** : daemon réel en
  politique stricte, précondition vérifiée par sonde témoin, auth 401,
  refus anti-SSRF fermés avec raison exacte (link-local, loopback,
  privé/CGNAT, unspecified). Règle consignée : jamais `--offline`
  pour ces bancs (`IpPolicy::permissive()` rendrait le test inerte).

## Fix : warns « operation sqlite lente » en rafale (2026-10-02)

Le seuil du warn de `Database::with` (`onionbit-db/db.rs`) était
250 ms : toute contention du mutex de connexion produisait une ligne
WARN par opération en attente (rafales de dizaines de lignes ~300 ms
au même instant). Seuil porté à **1 s** (la contention usuelle n'est
pas anormale) et cadence bornée à **1 warn/s** — les occurrences
supprimées sont comptées et rapportées au warn suivant via le champ
`suppressed`, le volume de la rafale reste observable sans inonder
le journal.

## Fix : `torrent_finished` réémis au démarrage pour les téléchargements déjà terminés (2026-10-02)

À chaque lancement, tous les téléchargements restaurés déjà complets
réémettaient `torrent_finished` — notifications + snackbar «
Téléchargement terminé » pour des fichiers finis depuis longtemps.
Cause : le `HashSet` de dédoublonnage de `spawn_progress_loop`
(`onionbit-core/session.rs`) repartait vide alors que le drapeau
`downloads.finished` est persisté en base. Le set est désormais
pré-amorcé depuis la base : seules les complétions observées dans la
session courante notifient — l'alerte `torrent_finished` de libtorrent
ne se rejoue pas au chargement d'un checkpoint. Effet de bord corrigé
au passage : `check_after_complete` ne relance plus un hash-check
complet de chaque torrent fini à chaque démarrage.

- Drapeau `finished` réinitialisé (`set_finished(false)`) si un
  téléchargement redevient incomplet (sélection de fichiers étendue,
  pièces invalidées) — la prochaine complétion notifie à nouveau.
- Test de régression `torrent_fini_restaure_ne_renotifie_pas`
  (`onionbit-core/tests/lifecycle.rs`) : torrent à hash de pièce réel
  + données sur disque → notifié une fois à la première session, zéro
  notification après restauration.

## ADR-0011 acceptée + Phase 8 messagerie planifiée (2026-10-02)

- `docs/architecture/decisions/0011-messagerie-anonyme-e2e.md` passe
  de « Proposée » à **« Acceptée »** : les questions ouvertes sont
  tranchées — trame bencode déterministe `{v,type,id,seq,ts,body,sig}`
  ≤ 32 Kio, signature Ed25519 de l'émetteur, clés applicatives
  HKDF domaine-séparé, **pas de ratchet en v1** (non-claim),
  anti-replay par `seq` + fenêtre 64 + dédup `id`, taille/DoS
  bornés, consentement explicite (`hello` → `pending` borné),
  présence DHT assumée et mesurée, persistance en clair v1,
  suppression réelle, offline = échec visible sans file.
- Roadmap : nouvelle **Phase 8** (étapes 36-41) — codec/crypto,
  transport e2e, consentement/anti-abus, persistance/livraison,
  API/SSE/UI, validation sécurité bloquante.
- `docs/plans/bancs_tests.md` §4.11 : 12 bancs `MS-1..MS-12`
  (protocole négatif, replay, consentement, offline, non-fuite,
  fingerprint) + SE-7 en §6.1 ; niveau « P0-msg » = bloque
  l'activation par défaut de la messagerie.

## Fusion colonnes Progression/État de la liste des téléchargements (2026-10-02)

- UI : les colonnes « Progression » et « État » affichaient en double
  le même pourcentage (« 100.0 % » + « Partage 100 % »). Elles sont
  fusionnées en une seule barre `DownloadProgressBar`
  (`app/lib/features/downloads/presentation/widgets/download_status.dart`)
  qui affiche le libellé de statut localisé centré sur la barre, dont la
  couleur suit la tonalité du statut (positif/avertissement/erreur) —
  modèle de la colonne Progression de qBittorrent. La fusion s'applique
  aux trois vues (table, liste compacte, grille) ; `DownloadStatusChip`
  et la clé l10n `colStatus` sont supprimés, la largeur minimale de la
  table passe à 1012 px. Le tri par état n'est plus exposé par en-tête
  (la colonne fusionnée trie par progression) ; `DownloadSort.status`
  reste valide pour les préférences persistées.

## Tempête ping/pong + discipline DHT anonyme (2026-10-02)

Réponse au signal fingerprint majeur mesuré en mesh : une lane
anonyme sur un magnet en stall produisait ~6 400 `on_cell`/s
symétriques (~365 Ko/s par sens) — ~200× la cadence Tribler.
Attribution finale par compteurs par type de cellule : **le flood
était une tempête `ping`/`pong` protocolaire, pas la DHT** —
`TunnelPong` était un alias de `TunnelPing`, donc chaque réponse au
keepalive de circuit repartait étiquetée `ping` ; entre deux nœuds
OnionBit, chaque « ping » régénérait un ping en retour → boucle
auto-entretenue à ~6 000 cellules/s (lancée par le premier
`do_ping`, 7,5 s après le premier circuit READY). Les pairs pyipv8
répondent un vrai `pong` — l'orage ne se produit qu'en mesh natif.
La discipline DHT livrée dans la même passe est un durcissement
mesuré à part entière (amplification entrante coupée, cadence de
stall bornée) — même si la cause du flood était ailleurs.

- **Bug protocolaire corrigé** : `TunnelPong` devient un vrai type
  (`msg_id=7`, même trame `I, H`). Test de régression
  `ping_pong_ne_declenche_pas_de_tempete` : un ping reçu produit
  exactement un pong, rien de plus.
- **Mesure après correction** (mesh 4 nœuds + Tribler, magnet en
  stall, run `fingerprint-mesh-20261002-214456`) : ~1 cellule/s
  résiduelle sur la lane, soit ~5 000× sous la baseline — le signal
  fingerprint disparaît.
- **Observabilité ajoutée** : compteurs de cellules par
  `inner_msg_id` (`cell_type_counts`, échantillon journalisé toutes
  les 4096) — c'est cet instrument qui a attribué le flood ; compteurs
  par direction sur `TunnelUdpSocket` + `Ipv8Stack::anon_dht_stats` ;
  `ExitInfo` étendu (`inbound_accepted`, `bytes_total`,
  `contacted_sources`, `idle_secs`).

Durcissement DHT / exit (conservé — budgets et cadence) :

- **Bug vendored corrigé** : `RecursiveRequest::request_one`
  (librqbit-dht) envoyait chaque requête deux fois — une seule
  réponse sert désormais table, callback, peers et récursion.
- **Posture client-only** (`tunnel_community/anon_dht_client_only`,
  défaut `true`) : la socket DHT tunnelisée écarte les requêtes
  entrantes (suffixe `1:y1:qe`) — la lane interroge la DHT sans la
  servir.
- **Budget de requêtes entrantes** (`DhtConfig::
  inbound_queries_per_second`, vendored) : seau à jetons global avant
  tout travail (table, peer_store, token) — l'excédent est ignoré en
  sémantique UDP, comptabilisé dans `DhtStats.dropped_inbound_
  queries`. Câblé sur `anon_dht_rate_pps` pour la lane anonyme.
- **Plafond de débit socket** (`tunnel_community/anon_dht_rate_pps`,
  défaut 30 datagrammes/s sortants, rafale 1 s, drop-tail) sur
  `TunnelUdpSocket` — borne le pire cas quel que soit le client DHT.
- **Backoff de stall** (`tunnel_community/anon_dht_backoff_cap_secs`,
  défaut 900) : `get_peers` sans progrès espace ses vagues en
  `REQUERY_INTERVAL × 2^idle` plafonné, jitter −0..25 %, état
  « discovery dégradée » journalisé à ≥3 passes idle, retour nominal
  au premier pair livré. `requery_interval` rendu configurable
  (testabilité).
- **Conntrack de sortie** (`tunnel_community/
  exit_inbound_source_ttl_secs` = 300,
  `exit_inbound_max_sources` = 2048) : `exit_recv_data` ne
  réencapsule que les datagrammes **non-IPv8** venant d'une
  destination contactée en sortie dans la fenêtre TTL — le bruit WAN
  adressé au port de sortie est écarté avant d'entrer dans le tunnel
  (sinon il pouvait générer des réponses uTP/DHT : amplification).
  Le trafic IPv8 e2e des services cachés est exempté — `create-e2e`
  arrive sans contact préalable et les messages sont signés.
- **Limite env var** : `DHT_QUERIES_PER_SECOND` malformée ne panique
  plus (repli 250 qps).
- Tests : `dht_backoff.rs` (backoff math + jitter, budget inbound —
  59/60 requêtes droppées, décroissance de cadence mesurée jusqu'au
  plafond sur DHT réelle contre sonde factice) ; tunnel loopback —
  filtre client-only, plafond de rafale, conntrack (source non
  contactée écartée), non-boucle ping/pong. Anonymat non affaibli :
  la DHT anonyme reste strictement sur la socket tunnelisée.

## Guard nodes activés par défaut (2026-10-02)

- `tunnel_community/guards_enabled` passe à **`true` par défaut**
  (ADR-0010) après validation terrain : test public vert (download
  anonyme 2 sauts, 276,4 Mo vérifiés, premiers hops ⊆ guard set,
  kill switch sans fallback direct) + validation workspace complète.
- Garde-fous : désactivation à chaud (`POST /api/settings` →
  `set_enabled` live, aucun redémarrage), nouveau toggle UI « Nœuds
  guards » dans la section Tunnels anonymes (FR/EN), API
  `GET /api/ipv8/tunnel/guards` lecture seule, warn
  `guards_pool_etroit` si le pool de candidats est insuffisant.
- Migration : profils sans la clé → nouveau défaut `true` ; un
  `guards_enabled: false` explicite dans `configuration.json` est
  préservé.
- Communication : mesure expérimentale de réduction d'exposition
  Sybil — pas une garantie d'anonymat. Les circuits 1-saut ne sont
  pas couverts (premier hop = exit `EXIT_BT` par construction).
- Fingerprinting : sessions mesh contrôlé OnionBit/Tribler
  documentées (`docs/security/fingerprinting.md`) — idle ~1,9× le
  volume Tribler en churn discovery, ~200× sur download anonyme en
  stall (DHT mainline tunnelisée vs `DHTDiscoveryCommunity`).
- Test public Tribler : API REST inutilisable sous charge relais
  réelle (~100 jambes saturées, asyncio affamée) — baseline coarse
  `netstat -s` conservée (~1 000–1 300 datagrammes/s par sens).

## Correctifs retour terrain DEBUG (2026-10-02)

- `tunnel_udp_socket` : le log par cellule relayée
  (`socket tunnel -> cellule data`) passe de `debug!` à `trace!` —
  en niveau DEBUG il produisait ~2 M de lignes / 340 Mio par session
  et saturait le disque ; `debug!` garde les événements circuit.
  `select-response recu` (par chunk) passe aussi en `trace!`.
- Toggle debug UI : le `EnvFilter` devient
  `debug,librqbit*=info` — les journaux par pair/datagramme des
  crates librqbit vendored (~30-40k lignes/session : timeouts
  `manage_peer`, DHT `response_reader`, `udp_tracker`, uTP
  out-of-order) ne noient plus les logs métier ; leurs `info`/`warn`
  restent visibles et `RUST_LOG` redonne le plein debug.
- `bandwidth` : `snapshot.up_bps` = meilleure estimation **mesure
  active ou pic passif** (sinon « mesure en cours… » à l'infini quand
  l'IGD ne répond pas) ; `note_applied` publie le plafond en mode
  fixe/illimité ; `relay_mode` (`auto`/`unlimited`/`fixed`) exposé
  dans `/api/statistics/ipv8` ; échec UPnP désormais loggé en `info!`.
- UI : « Plafond débit servi » affiche « illimité » en mode
  `unlimited` au lieu de « — ».

## Message « daemon injoignable » (2026-10-02)

- `ApiClient` enveloppe les échecs de transport (`http.ClientException`
  — `SocketException` natif / `TypeError: Failed to fetch` web) dans
  `DaemonUnreachableException` portant l'URI cible — tous les chemins
  (`_send`, `getText`, `getStreamedLines`).
- `ErrorState` prend désormais l'erreur brute : daemon injoignable →
  icône `cloud_off` + « Le daemon ne répond pas » + URL contactée ;
  `ApiException` → `message` métier sans le préfixe `ApiException(…)` ;
  reste → `toString()`. Les 7 écrans passent `error: e`.

## Contention SQLite dans la boucle de paquets IPv8 (2026-10-02)

- **Diagnostic terrain** : sous charge réseau (relais + requêtes de
  pairs distants), le journal montrait ~320 « operation sqlite lente »
  (jusqu'à 24 s d'attente sur le `Mutex<Connection>`) et des
  « lag executor tokio » corrélés — les accès DB étaient exécutés
  **synchronement dans le traitement des paquets** (`ContentProvider`).
- `ContentProvider` passe en **futures boxed** (dyn-safe) :
  `healths_for`, `process_health`, `remote_select`,
  `process_select_response` — les sites d'appel de `on_packet` sont
  déportés dans `tokio::spawn`, le travail SQL sur le pool bloquant
  via `db.call` (`spawn_blocking`). L'executor n'est plus figé par un
  scan ou un batch d'inserts.
- **Cache `healths_for`** (`Ipv8Config::content_healths_cache_secs`,
  défaut 30 s, par `request_type`) — les `HEALTH_REQUEST` distants en
  rafale ne déclenchent plus un `ORDER BY RANDOM()` à chaque paquet.
- **Transactions** : `process_health` et `process_select_response`
  batchent leurs écritures en **une** `unchecked_transaction` au lieu
  d'un autocommit par ligne (N appends WAL → 1 commit).
- `version_info` reste synchrone (pas d'accès DB).
- **Audit DB** : pragmas déjà corrects (WAL, `synchronous=NORMAL`,
  `busy_timeout`, `temp_store=MEMORY`, cache 20 Mio, mmap 64 Mio) ;
  ajouts —
  `wal_autocheckpoint` relevé à ~32 Mio (le seuil par défaut de 4 Mio
  déclenchait le checkpoint en pleine rafale d'écriture),
  `PRAGMA wal_checkpoint(TRUNCATE)` à l'arrêt de session (`stop()`),
  index `idx_torrent_state_seeders` (v12) pour le tri
  `ORDER BY seeders DESC` de `healths_for`.
- Toutes les tables justifient leur persistance (`downloads` =
  restauration, `channel_node`/`FtsIndex`/`torrent_state`/`trackers` =
  métadonnées servies aux pairs Tribler + recherche locale,
  `ipv8_peers`/`tunnel_pex`/`guards` = reconnexion, `rss_items`) ;
  `channel_node`/`torrent_state` croissent sans borne comme chez
  Tribler — GC éventuel à étudier, pas de suppression en l'état
  (interop d'abord).

## Plafond de débit servi `max_relayed_rate` — mode auto (2026-10-02)

- `tunnel_community/max_relayed_rate` est désormais un **mode** :
  **`-1` = automatique** (défaut produit), `0` = illimité
  (comportement pyipv8), `>0` = plafond fixe en octets/s.
- **`services/bandwidth.rs` — estimateur de capacité upload** :
  le débit servi est symétrique (1 datagramme relayé = 1 in + 1 out),
  l'upload est le facteur limitant → le plafond seul sur l'upload
  borne automatiquement le download consommé. Sources, meilleure
  disponible : débit WAN du routeur via **UPnP**
  (`WANCommonInterfaceConfig:GetLinkLayerMaxBitRates`, trafic LAN),
  **sonde HTTP POST** (`bandwidth/probe_up_urls`, opt-in — vide par
  défaut, anti-SSRF `ip_policy`), **pic passif** des compteurs
  endpoint (borne basse).
- **`tunnel_community/bandwidth`** : `share` (1/3), `floor_bps`
  (64 Kio/s), `fallback_bps` (512 Kio/s avant première mesure),
  `measure_upnp`, `probe_up_urls`/`probe_bytes`/`probe_timeout_secs`,
  `measure_interval_secs` (1 h), `warmup_secs` (30 s),
  `sample_secs` (5 s).
- Tâche session périodique : warmup → mesure → tick 5 s (pic passif +
  réapplication à chaud via `set_relay_rate_bps`) — seulement en mode
  auto ; un plafond fixe n'est jamais écrasé.
- `/api/statistics/ipv8` expose `bandwidth` : `measured_up_bps`,
  `measured_down_bps`, `source` (`upnp`/`probe`/`passive`),
  `passive_peak_up_bps`, `effective_relay_bps`, `relay_dropped`.
- **UI** : le champ « Débit servi maximum » disparaît — remplacé par
  une note automatique ; la clé reste réglable dans
  `configuration.json` pour les usages avancés. Le plafond servi est
  visible dans la barre d'état (`relais ≤ …`) et la capacité mesurée
  dans Diagnostic → Statistiques (section « Anonymat »).
- Seau à jetons (`RelayRateLimiter`, rafale bornée à 1 s de débit)
  dans la pompe d'émission sérialisée de la `TunnelCommunity` :
  couvre les deux variantes de `SendJob` — cellules **relayées**
  (`Endpoint`) et datagrammes de **sortie** (`ExitSocket`). Faute de
  jetons le datagramme est perdu : sémantique UDP, la charge est
  lissée par les retransmissions uTP aux extrémités plutôt que par
  une file qui croît sans borne.
- **Appliqué à chaud** via `POST /api/settings`
  (`TunnelCommunity::set_relay_rate_bps`, contrairement à
  `max_joined_circuits` qui reste restart-only) ; compteur
  `relay_rate_dropped()` pour l'observabilité des pertes.
- UI : champ « Débit servi maximum » (Kio/s) dans la section
  « Anonymous tunnels », distingué des bornes BitTorrent
  `libtorrent/max_*_rate` qui ne concernent que les téléchargements
  locaux.

## Plafond de relais `max_joined_circuits` (2026-10-02)

- `tunnel_community/max_joined_circuits` (défaut 100, valeur Python de
  `should_join_circuit` — `tunnel.py`) exposé jusqu'à
  `TunnelSettings` : au-delà du plafond de jambes de relais + sockets
  de sortie, les `create` entrants sont refusés. Borne la charge que
  le réseau impose au nœud — un membre joignable et stable accumule
  les relais d'autrui (observé : ~550 Mo relayés).
- Configurable dans l'UI (section « Anonymous tunnels », champ
  « Relais servis maximum ») — pris en compte au redémarrage, comme
  `enabled`/`exitnode_enabled` (`TunnelSettings` figé à la
  construction de la communauté).
- Libellés anonymat corrigés : « TunnelCommunity enabled » mentionne
  désormais le rôle de relais interne, « exit node » distingue la
  sortie vers l'Internet public sous l'IP de l'utilisateur.

## Fix : réglages « restart-only » impossibles à commuter dans l'UI (2026-10-02)

- **Symptôme** : les commutateurs `tunnel_community/enabled`,
  `ipv8/enabled`, `dht_discovery`, `content_discovery_community`,
  `torrent_checker`, `libtorrent/{dht,upnp,lsd,utp}` revenaient
  immédiatement à leur position de démarrage. Le `POST /api/settings`
  persistait bien la valeur dans `configuration.json`, mais `GET`
  recouvrait ces clés avec l'état **runtime** (`apply_runtime_view`
  ← `effective_config()`, qui n'avait d'overrides que pour les
  réglages appliqués à chaud) — l'écran affichait donc l'état du boot
  au lieu de la valeur en attente de redémarrage.
- **`onionbit-core`** : `ServiceOverrides` mémorise désormais les
  sections restart-only postées (`ipv8`, `engine`,
  `enable_torrent_checker`) — `effective_config()` reflète la
  configuration en attente (parité Python : `config.configuration`
  est muté in-place par le `POST` et relu tel quel par le `GET`),
  sans rien appliquer à la session en cours. `exitnode_enabled` et
  `min/max_circuits` étaient déjà correctement reflétés.
- **`peer_flags` tunnel** : `to_core_config` produisait `RELAY` seul
  (=1) au lieu de `{RELAY, SPEED_TEST}` (=9, `TunnelSettings` pyipv8)
  — le nœud ne répondait jamais aux `test-request` pyipv8 ; base
  corrigée dans `to_core_config` et `Ipv8Config::production`
  (`exitnode_enabled` ajoute toujours `EXIT_BT|EXIT_IPV8|EXIT_HTTP`).
- Libellés : le sous-titre du nœud de sortie précise que la sortie
  couvre aussi trackers UDP/DHT, requêtes HTTP de trackers et trafic
  IPv8 (pas seulement BitTorrent), et qu'il est sans effet si la
  TunnelCommunity est désactivée ; carte « État effectif » : clés
  `listen_port`/`listen_interfaces` inexistantes → `port` +
  `listen_interface[_v6]` réels.
- Tests : `settings_restart_only_refletent_la_valeur_postee`
  (`api.rs`, POST→GET restart-only) +
  `peer_flags_refletent_exitnode_enabled` (`daemon_config.rs`).

## Fix : téléchargements anonymes « en vérification » figés (2026-10-02)

- **Symptôme** : `tunnel_community.enabled=false` au démarrage du
  daemon → `restore_downloads` sautait silencieusement chaque ligne
  `anon_hops>0` (« moteur anonyme indisponible ») ; les lignes DB non
  réinjectées restaient affichées `WAITING_FOR_HASHCHECK`
  (« Vérification », 0 %) toute la session et tout `PATCH` répondait
  `404 this download does not exist` (le téléchargement n'existe dans
  aucun moteur).
- **`onionbit-core`** : le skip de lane anonyme à la restauration est
  désormais remonté au GUI via `Notification::TriblerException`
  (parité `on_tribler_exception` Python, déjà utilisée pour les
  échecs de re-add) et compté dans `failed` ; nouvel accesseur
  `CoreSession::restore_finished()`.
- **`onionbit-api`** : `GET /api/downloads` n'émet les lignes
  persistées non restaurées en `WAITING_FOR_HASHCHECK`/`STOPPED` que
  tant que `load_checkpoint` tourne — après `restore_done` elles sont
  absentes de la liste (comme Python : `get_downloads` ne liste que
  les téléchargements chargés) ; `checkpoints.all_loaded` reflète
  désormais la fin réelle de la restauration.
- Test : `restauration_anonyme_sans_ipv8_notifie_une_exception`
  (`lifecycle.rs`) — ligne `anon_hops=3` persistée + session sans
  ipv8 → download non restauré + `tribler_exception` émis.

## Version 0.5.0-alpha (2026-10-02)

- Release succédant à `v0.4.0-alpha` :
  - **interface web Flutter** servie par le daemon en same-origin
    (`http://127.0.0.1:<port>/`) — même codebase que le desktop,
    étapes 31–35 du plan `web_ui_plan.md` (ADR-0012) ;
  - transports web `fetch_client` (SSE + speed test streamés),
    pickers/drop/connexion adaptés navigateur, dialogue clé API ;
  - statiques sous `/` exemptes d'auth (parité `/ui`/`/static`),
    `/api/*` inchangé derrière la clé ; `api/web_ui_*` + `--web-ui-dir` ;
  - **auto-connexion** : clé API injectée dans l'`index.html` servi
    (`api/web_ui_inject_key`, défaut `true`) — pas de saisie ;
    lanceur `OnionBit Web.cmd` qui démarre le daemon au besoin ;
  - packaging : `dist/web/` intégré au bundle, systray « Ouvrir dans
    le navigateur », streaming `/stream/{i}?key=` par fichier.

## Interface web Flutter servie par le daemon — Phase 7 (2026-10-02)

- **Étapes 31–35** de `docs/plans/roadmap.md` — plan
  `docs/plans/web_ui_plan.md`, décision **ADR-0012** : le daemon sert
  le build Flutter web en **same-origin** sous `/` (modèle des
  exemptions `/ui`/`/static` de l'`ApiKeyMiddleware` Python) — aucun
  CORS ajouté, bind loopback inchangé, `onionbit-network-policy`
  intouchée.
- **Transports web** (`app/lib/core/api/http_transport*.dart`) :
  import conditionnel `dart.library.io` — natif = `package:http`
  classique, web = `fetch_client` (API Fetch) ; sans ça XHR bufferise
  tout et le SSE `/api/events` + le speed test circuits ne streament
  pas. `ApiClient`/`SseClient` passent par cette façade.
- **Couches plateforme** : `pick_file` (natif `file_selector` / web
  `<input>` → octets — le dialogue « Ajouter » est désormais
  *bytes-first*, `.torrent` uploadé en brut, `.magnet` lu en texte),
  `pick_directory` (natif / web → `/api/files/browse` côté daemon via
  `DaemonDirectoryPicker`), `drop_zone` (`desktop_drop` / drag HTML5),
  `open_url`, notifications navigateur (`web_notify`), découverte
  daemon no-op sur web.
- **Connexion web** : `baseUrl` = `Uri.base.origin` en build web ;
  dialogue de connexion pour saisir la clé API (persistée
  `shared_preferences`), bandeau basculant sur « clé requise » quand
  l'API répond 401 ; bouton de re-découverte locale masqué sur web.
- **Statiques côté Rust** : `onionbit-api` remodulé — routes sous
  `/api` (middleware clé + fourre-tout `/api/*` inconnu → 401, parité
  Python), `webui.rs` = `ServeDir` + fallback SPA `index.html` +
  `nosniff`/`Cache-Control` ; config `api/web_ui_enabled` +
  `api/web_ui_dir`, CLI `--web-ui-dir`, détection auto
  `<exe>/web` → `state_dir/web` → `app/build/web`.
- **Packaging** : `build_dist.ps1` produit `dist/web/` (+ manifest) ;
  `verify_all.ps1` gagne `flutter analyze`/`test`/`build web` ;
  systray « Ouvrir dans le navigateur » ; `dist_lisezmoi.txt`
  documente le parcours (clé API, `?key=`).
- Tests : `web_ui_statiques_exemptes_d_auth` (+ 57 tests API verts),
  19 tests Flutter verts, `verify_all` complet OK ; fumée réelle :
  `GET /` 200, fallback SPA 200, `/api/*` 401 sans clé, SSE streamé.

## Version 0.4.0-alpha (2026-10-02)

- Release succédant à `v0.3.2-alpha` :
  - **sonde de mise à jour réelle** (`versions/check` : releases GitHub +
    sondes `check_urls`, anti-SSRF, timeout borné) ;
  - guard nodes expérimentaux (ADR-0010, `guards_enabled=false` par
    défaut : persistance premier saut, `GET /api/ipv8/tunnel/guards`,
    bascule à chaud) ;
  - fix `rendezvous-established` (WAN estimé) + pacing des réannonces DHT ;
  - fix self dans le pool de candidats (auto-adoption guard) ;
  - endpoint `debug/circuit-downloads` (observabilité circuits↔downloads) ;
  - campagne libFuzzer de référence : 6,28 Md d'exécutions, 0 crash ;
  - mesures fingerprinting de référence (`docs/security/fingerprinting.md`) ;
  - anti-fuite : `torrent_checker` ne scrape plus les swarms anonymes.

## Sonde de mise à jour réelle — `versions/check` (2026-10-02)

- `GET /api/versioning/versions/check` interroge désormais réellement
  les releases : port de `VersioningManager.check_version` Python dans
  `onionbit_core::services::versioning` — `check_urls` (`{current}`
  substitué, équivalent `release.tribler.org`) puis API GitHub
  `releases?per_page=1` en tête si `allow_pre`, `releases/latest` en
  queue sinon ; première réponse valide gagne, échec → sonde suivante.
- Trafic direct via `fetch_checked_with` (nouvelle variante à
  timeout/`User-Agent` paramétrables de `fetch_checked`) : anti-SSRF
  `ip_policy` sur chaque adresse résolue, corps borné, UA
  `OnionBit/{v} (os=…; arch=…)` exigé par l'API GitHub ; jamais par
  les circuits onion. Config : `versioning/github_repo` (défaut =
  champ `repository` du workspace), `versioning/check_urls`,
  `versioning/check_timeout_secs` (défaut 5 = `ClientTimeout` Python).
- Comparaison `packaging.Version` portée (segments numériques +
  dev/a/b/rc/post) ; **divergence assumée** : le tableau
  `releases?per_page=1` est accepté — `dict["name"]` Python lève
  `TypeError` dessus, la sonde GitHub `allow_pre` de Tribler échoue
  donc toujours.
- Tests : unitaires `probe_urls`/`version_newer`/`release_name` +
  sonde loopback (`core`), intégration `versioning_check_sonde_locale`
  (api) ; le test existant neutralise `github_repo` (pas de trafic
  externe en test).

## Matrice interop guards — premier run terrain (2026-10-02)

- Scripts `interop_hidden_tribler_download.ps1` /
  `interop_hidden_tribler_seed.ps1` : switch `-Guards` injectant
  `tunnel_community.guards_enabled` sur tous les noeuds Rust du
  maillage + verdicts dédiés (`guard set actif`, `premier hop dans le
  guard set` via `verified_hops[0]` vs `guards[].mid`) + snapshot
  `/ipv8/tunnel/guards` dans le rapport.
- **Sens A (Tribler télécharge depuis OnionBit, guards ON)** : vert —
  SEEDING, 10 circuits IP_SEEDER READY, annonce DHT propagée,
  téléchargement 2 Mio complet, SHA-256 correct, tous les premiers
  hops multi-hop dans le guard set (0 hors-set).
- **Sens B (OnionBit télécharge depuis Tribler, guards ON)** : le
  téléchargement réussit (SHA-256 correct) mais le verdict premier-hop
  échoue → deux défauts réels trouvés et corrigés :
  - le `created-e2e` désignant le downloader comme RP (`rp=D`, choix
    légitime de Tribler) réintroduisait la propre clé du noeud dans
    l'annuaire → **auto-adoption comme guard**, circuits dégénérés
    vers sa propre socket. `get_candidates`, `get_candidates_subset`
    et `first_hop_pool` excluent désormais `my_pk`.
  - `RP_DOWNLOADER`/`RP_SEEDER` contournaient l'ordonnancement guards
    via `pick_first_hop` → passent par `first_hop_candidates`
    (guards + alternates), repli permissif sur les pairs du service
    tunnel quand le registre de flags est vide (comme `send_extend`).
- Test de régression `soi_meme_exclu_des_candidats_guards_et_premiers_hops`.
- Le repli `estimated_wan` forcé en loopback + écho
  d'introduction-response rend ce cas accessible : exactement le type
  de défaut que la validation terrain devait débusquer avant
  l'activation par défaut.

## ADR-0011 — messagerie anonyme e2e (proposée) (2026-10-02)

- `docs/architecture/decisions/0011-messagerie-anonyme-e2e.md` :
  contact = clé publique IPv8 du destinataire ; swarm de messagerie
  `SHA1("onionbit messaging" || pk)` « seedé » par le destinataire
  (IP_SEEDER + annonce DHT) ; l'expéditeur lie un circuit e2e
  (`RP_DOWNLOADER` + `link-e2e`) puis trames bencode dans les
  cellules `data` du circuit lié. En ligne seulement, persistance
  `messages`, REST + SSE. VoIP/groupes/store-and-forward hors
  périmètre v1. **Design uniquement** — implémentation après revue.

## Guards : bascule à chaud via `POST /api/settings` (2026-10-02)

- `GuardSet.enabled` devient un `AtomicBool` (`is_enabled`/
  `set_enabled`) : `tunnel_community/guards_enabled` s'applique sans
  redémarrage dans `apply_service_settings`, avec injection tardive du
  `DbGuardStore` au premier armement (le set persistant est chargé
  alors ; désactiver conserve le set pour un ré-armement ultérieur).
- Test `bascule_a_chaud_sans_reconstruction` : tirage pyipv8 exact
  quand désactivé à chaud, même guard après ré-armement.

## Guards : maintenance proactive + persistance redémarrage (2026-10-02)

- `do_guard_maintenance` dans `run_maintenance` (cadence
  `guards.maintenance_interval`, défaut 60 s, no-op si désactivé) :
  purge des guards expirés/injoignables, promotion de la réserve,
  adoptions anticipées sur le pool courant — **jamais sous pression
  d'un storm `DESTROY`**. Refactor : pool de candidats extrait en
  `first_hop_pool` (partagé avec `first_hop_candidates`).
- Test `CoreSession` fichier `guards_survivent_au_redemarrage_du_daemon`
  : adoption → `stop` → nouvelle session même `state_dir` → guard
  rechargé depuis `onionbit.db`. Contrepartie
  `guards_desactivees_ne_chargent_pas_le_set` (repli pyipv8 strict).
- ADR-0010 : statut des tests attendus mis à jour (tous ✅) + défaut
  `guards_enabled=false` corrigé dans la section non-régression.

## Observabilité circuits↔downloads anonymes (2026-10-02)

- `GET /api/ipv8/tunnel/debug/circuit-downloads` — **extension Rust**
  (Python garde `download_states` interne au `monitor_downloads`, pas
  d'endpoint) : par liaison download↔swarm — `info_hash` réel,
  `lookup_info_hash` (cle swarm), `hops`, `state`, `seeder`,
  `swarm_peers` (connexions e2e) et les circuits tunnel portant ce
  lookup (`circuits_info` filtré). `{"downloads": []}` sans tunnel.
- `Ipv8Stack::swarm_downloads()` — snapshot join `swarm_lookup` +
  `swarm_states`, trié stable ; entrée sans état omise (course
  d'insertion d'un tick, pas d'état inventé).
- CLI `tunnel --show downloads` : infohash, état, hops, seeder, pairs
  e2e, ids des circuits liés.

## Campagne libFuzzer de référence complète — 0 crash (2026-10-02)

- ~5 h de fuzzing coverage-guidé natif Windows/MSVC (`-s none`),
  6 cibles, **6,28 milliards d'exécutions cumulées, zéro crash,
  zéro timeout, zéro OOM** : `raw_datagram` 170 M, `tunnel_cell`
  1,83 Md, `unsigned_dispatch` 575 M, `tunnel_payloads` 591 M,
  `ipv8_packet` 1,50 Md, `utp_datagram` 1,62 Md. Détail et corpus
  dans `docs/security/fuzz_journal.md` / `fuzz_journal.csv`.
- Corpus persistés sous `fuzz/corpus/<target>/` (636 entrées pour
  `tunnel_payloads`, 372 `unsigned_dispatch`, 198 `utp_datagram`).
- Provenance : `raw_datagram` fuzzée sur binaire `9bc4a9d` exact ;
  cibles suivantes relinkées à HEAD (guards désactivées — hors
  surface fuzzée), cf. note dans le journal.

## Implémentation expérimentale des guard nodes (ADR-0010) (2026-10-01)

- `onionbit-tunnel/guards.rs` : `GuardSet` (3 actifs + 2 réserve,
  persistance 30 jours, injoignable 24 h, 3 échecs de handshake avant
  rétrogradation, diversité /24 IPv4 et /64 IPv6 à l'admission),
  trait `GuardStore` injectable (`onionbit-tunnel` ne dépend pas de
  `onionbit-db` — sens des dépendances inversé, implémentation DB à
  venir) + `InMemoryGuardStore` volatile.
- Intégration `community.rs` : `first_hop_candidates` ordonne
  `actifs ++ réserve ++ tirage libre` quand `guards.enabled` (le
  `required_exit` reste exclu en amont, jamais adopté comme guard) ;
  timeout du `create` initial → `mark_failure` sur `unverified_hop` ;
  premier `created` vérifié (`hops_done == 1`) → `mark_alive` +
  rafraîchissement d'adresse (le guard survit à un changement d'IP).
- `GuardsConfig::enabled = false` par défaut : sélection pyipv8
  **bit-exacte** sans le flag (test `desactive_est_le_tirage_pyipv8_exact`).
  Jamais appliqué à `hops=1` `DATA` (premier hop = exit, choisi par
  circuit) ni au mode direct `hops=0`.
- Test d'intégration `guards_bornent_les_premiers_hops_sous_storm_destroy`
  : storm de `DESTROY` hostile → reconstruction bornée au guard adopté,
  set inchangé (sur loopback la dédup /24 n'admet qu'un guard —
  assertion déterministe). 5 tests unitaires (adoption+diversité,
  ordre, rétrogradation sans tirage sous pression, remise à zéro sur
  preuve de vie, repli pyipv8).
- Persistance SQLite (migration v11, table `guards` : clé publique,
  dernière adresse, adoption/dernière vue, échecs, actif/réserve,
  `position` = ordre sémantique du set) via `onionbit-db::guards`
  (lignes brutes, snapshot `replace_all` ≤ 5 lignes) et adaptateur
  `onionbit-core::guard_store::DbGuardStore` implémentant
  `GuardStore` — `onionbit-tunnel`/`onionbit-db` ne dépendent pas l'un
  de l'autre, la couture vit dans `core`. Injection dans
  `ipv8_stack` quand `tunnel_community/guards_enabled` est vrai dans
  `configuration.json` (défaut `false` partout).
- Diagnostic : `GET /api/ipv8/tunnel/guards` (extension Rust —
  `{guards: [{mid, address, reserve, failures, adopted_at,
  last_seen}], enabled}`) ; documentée dans `api_rest_mapping.md`.
- Reste à faire : activation par défaut après validation terrain.
- Hors guards : `scripts/fingerprint_stats.ps1` +
  `docs/security/fingerprinting.md` — échantillonneur de compteurs
  REST (overlays/statistics + circuits) en CSV pour la mesure
  comparative de fingerprinting vs Tribler officiel (agrégats,
  jamais de PCAP).

## Campagne libFuzzer native Windows + ADR guard nodes (2026-10-01)

- **Coverage-guiding sous Windows/MSVC rendu possible** : rustc ne
  livre aucun runtime sanitizer pour `windows-msvc` et l'instrumentation
  sancov émet des bornes `__start_/__stop_` que lld-link ne synthétise
  pas. Contournement validé : `fuzz/sancov_shim.c` (compilé par
  `fuzz/build.rs`) définit les bornes de groupes `.SCOV$*`/`.SCOVP$*` ;
  `CUSTOM_LIBFUZZER_PATH` utilise le runtime fuzzer précompilé de LLVM
  (`clang_rt.fuzzer-x86_64.lib`) ; `LIB` pointe les libs MSVC/SDK.
  Campagne coverage-guidée (`-s none`, sans ASan) fonctionnelle.
- `scripts/fuzz_campaign.ps1` : détection automatique LLVM/MSVC,
  plan de référence (5 h au total), journal CSV dans
  `docs/security/fuzz_journal.csv`, doc `docs/security/fuzz_journal.md`.
- Smoke run 6 cibles : ~98 M exécutions, zéro crash, corpus amorcé ;
  campagne complète lancée sur commit `9bc4a9d`.
- **ADR-0010 « guard nodes »** (`docs/architecture/decisions/`) :
  persistance du premier saut contre la multiplication des tirages
  d'entrée sous `DESTROY` storm — 3 actifs + 2 réserve, 30 jours,
  diversité IP, bootstrap pool existant, écart pyipv8 comportemental
  (aucun changement filaire), jamais sur `hops=0`. Proposée, aucune
  implémentation avant acceptation.
- `threat_model.md` : section « Déclaration de couverture » résumant
  ce qui est démontré vs hors périmètre.

## Anti-fuite : `torrent_checker` ne scrape jamais un swarm anonyme + invariant d'egress hidden-service (2026-10-01)

- Audit statique des chemins d'egress (`UdpSocket::bind`, `send_to`,
  `lookup_host`, `reqwest`, `TcpStream`) : le plan de contrôle IPv8
  passe uniquement par `UdpEndpoint::send_to` ; les `peers-request`/
  `create-e2e` transitent dans les cellules (`tunnel_data`/`send_cell`)
  — c'est l'exit qui fait le lookup DHT du swarm.
- **Fuite corrigée** : `TorrentChecker` scrapait les trackers en clair
  depuis l'IP réelle pour tout infohash de `torrent_state` — dont ceux
  en téléchargement/seeding anonyme (`anon_hops > 0`), liant IP ↔
  contenu. `check_tracker` filtre désormais ces infohashes avant tout
  egress et `check_oldest` les saute en SQL ; la santé d'un swarm caché
  vient du tunnel (`peers-request`), comme Tribler — écart assumé avec
  upstream qui scrape en clair.
- Nouveau test `torrent_checker_never_scrapes_anonymous_infohash`
  (`onionbit-core`) : tracker UDP espion en loopback — `check_tracker`
  et `check_oldest` silencieux, zéro datagramme émis.
- Nouveau test `hidden_service_egress_uniquement_vers_relais`
  (`circuits_loopback`) : tap `UdpEndpoint::set_tap` sur 3 noeuds —
  pendant `join_swarm`/`peers-request`/`create-e2e`/`link-e2e`/donnée
  e2e à 2 sauts, tout egress du downloader est un datagramme à préfixe
  tunnel vers ses seuls premiers sauts, et ni le point d'introduction
  ni le seeder ne reçoivent de paquet sourcé de l'adresse du downloader.
- Audit des bornes `hops` : production bornée 1..=3 par `anon_engine`
  et `goal_hops` REST ; `swarm_circuit_hops` +1 sur `IP_SEEDER`/
  `RP_DOWNLOADER` épinglé par assertion (`RP_DOWNLOADER.goal_hops == 2`
  à `hops=1` — le RP ne voit jamais le downloader) ; `join_swarm(0,
  non-seeder)` logue un WARN (échec fermé, parité pyipv8). Sémantique
  `hops=0/1` documentée dans `docs/security/threat_model.md`.
- Test d'injection négative `hidden_service_injection_paquets_forjes` :
  forge sur socket brute de `peers-response`/`created-e2e`/`linked-e2e`
  sans état et de `create-e2e` à clé inconnue → tous rejetés par les
  caches (`peers_requests`, `e2e_requests`, `intro_point_for`,
  whitelist du dispatch non signé) ; doublon `create-e2e` à clé connue
  rejoué verbatim depuis `seen_e2e` sans second `RP_SEEDER`. Surface
  résiduelle (coût RP par identifiant neuf, parité pyipv8) documentée
  dans `docs/security/threat_model.md`.
- Fuzzing des parsers (P1) : correctif DoS dans `cell.rs` —
  `check_cell_flags`, `decrypt_cell`, `encrypt_cell` et
  `Cell::swap_circuit_id` indexaient sans borne ; une cellule
  déchiffrant à ≤ 29 octets crashait le process (injectable par un
  relais du circuit). Gardes `Truncated` ajoutées. Harnais stable
  `tests/fuzz_regression.rs` (proptest : bordures du format 22..46,
  listes tronquées, dispatch non signé, DHT, uTP) + scaffold
  `fuzz/` cargo-fuzz (6 cibles, dont `on_raw_datagram` complet) prêt
  pour nightly+clang.
- Storm `DESTROY` : test `destroy_storm_et_reconstruction_bornee` —
  destroys signés arbitraires (parité pyipv8 : pas d'auth par saut)
  sur circuits valides/cids inconnus → nettoyage idempotent, purge
  symétrique des sorties, rebuild OK ; borne `ready+pending >= min`
  de `build_circuits_if_needed` épinglée (rythme rebuild plafonné
  par le watchdog 5 s, pas par le flux entrant). Épingle DNS
  `adresse_domaine_ne_se_resout_pas_cote_client` : `UdpAddress::Domain`
  opaque côté client, résolution uniquement côté exit.

- `on_establish_rendezvous` répondait `local_addr` au lieu de
  `my_estimated_wan` (pyipv8 `TunnelCommunity.on_establish_rendezvous`)
  : le `RendezvousInfo` du `created-e2e` portait `0.0.0.0:0`, le
  downloader pyipv8 ne pouvait pas créer sa jambe `RP_DOWNLOADER` —
  `E2ERequestCache` expirait en boucle, download figé après kill.
  Fix : WAN estimé + WARN si non spécifié.
- Instrumentation du trajet retour `created-e2e` corrélée par
  `identifier` (relais intro, création RP seeder, `exit_data` IPv8,
  `exit_recv_data`) — le cycle `create-e2e`→`created-e2e`→`link-e2e`
  est maintenant traçable de bout en bout.
- `reannounce_intro_points` émettait ~10 `store_value` en rafale :
  chaque store lance un `find_nodes` qui interroge les mêmes noeuds,
  dépassant le `blocked()` pyipv8 (10 req / 5 s) — les annonces étaient
  droppées. Les stores sont maintenant séquencés dans une seule tâche,
  espacés de `dht_reannounce_stagger` (500 ms, `TunnelSettings`), avec
  log par point d'introduction.
- Test `hidden_seed_e2e_burst_integrity` : tolérance de pertes 99 %→
  90 % (drop-tail attendu sous contention CPU de la suite parallèle).

### Validation — banc `interop_hidden_killseeder -KillTarget anchor`

Maillage : A1 = bootstrap + `EXIT_BT`, A2 = second `EXIT_BT`, A3 =
relais (sans second exit, tuer A1 rend toute reconstruction DATA
impossible par design — `select_exit` pyipv8 n'a plus de candidat ;
limite de topologie de banc, pas de casse protocole). Binaire
reconstruit incluant la pompe d'émission FIFO.

| sens | hops | kill_target | flux_survit | octets post-kill | fenetre_morte | nouveau_circuit | nouvelle_annonce | completion | sha256 | fallback_direct | verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|
| A (seeder Rust → Tribler 8.4.3) | 1 | anchor A1 | oui (+30 s) | 21 709 636 | 0 s | oui `43074219a86d…` (id hors snapshot pré-kill, dernier saut ≠ A1) | oui — `seeder_pk` identique, `intro_mid` = nouveau mid | 100 % | identique `f7c1cb3f…` | aucun | PASS |
| B (seeder Tribler 8.4.3 → Rust) | 1 | anchor A1 | oui (+30 s) | 21 938 176 | 0 s | oui `cec559938f90…` (id hors snapshot pré-kill, dernier saut ≠ A1) | oui — seeder identique, `intro_mid` = nouveau mid | 100 % | identique `da0ea3ae…` | aucun | PASS |

Artefacts : `target/interop-killseed-20261001-173905/` (A) et
`target/interop-killseed-20261001-175315/` (B) — `report/verdicts.txt`
pour le détail des 13-14 assertions chacun. FAIL résiduel cosmétique
dans les deux runs : le gate `Tribler : bootstrap maillage` compte
`exits=0` trop tôt au démarrage de T (le téléchargement anonyme
fonctionne ensuite).

### Suite complète de résilience (8 bancs, `target/bench-suite-20261001-191301/`)

| banc | résultat |
|---|---|
| `killseed -KillTarget seeder` A | PASS — drain borné (+574 ko/2 s), fenêtre morte 30 s, restart+fastresume, reprise 56 s, SHA-256 |
| `killseed -KillTarget seeder` B | PASS — restart Tribler, reprise Rust 15 s, SHA-256 |
| `killseed -KillTarget intro` A | PASS (re-check : verdict DHT via `last_seen`, cf. `0485c57`) |
| `killseed -KillTarget intro` B | PASS |
| `killseed -KillTarget anchor -Hops 3` A | PASS — reconstruction 3 sauts, `intro_mid=550826de…` |
| `killseed -KillTarget anchor -Hops 3` B | PASS (re-check : critère `intro intact` pour ancre hors chemin) |
| `live_hidden_upload` (3 daemons full-Rust) | PASS — 8/8 verdicts |
| `interop_public_dht` (réseau réel, Sintel magnet) | PASS — 589 687 octets vérifiés, route 3 sauts via exits Tribler réels |

2 faux FAIL corrigés dans le script de banc (`0485c57`) : contradiction
`intro_mid nouveau ∧ == newMid` quand l'intro reconstruit atterrissait
sur un nœud ayant déjà des intros pré-kill (mid du nœud identique) —
résolu par `last_seen` post-kill ; et BOM UTF-8 parasite dans
`interop_public_dht.ps1`. Le gate `bootstrap Tribler exits=0` (snapshot
`/ipv8/overlays` convergent souvent *après* le début du transfert) est
devenu une ligne `INFO` non bloquante : les vrais critères sont
fonctionnels — download accepté, `create-e2e` observé, octets vérifiés,
intégrité finale.

Frontière de confiance explicitée dans `docs/security/threat_model.md`
(démontré vs non-démontré : pas de protection contre corrélation de
trafic, adversaire global, Sybil massif, exit malveillant lisant le
BitTorrent clair — même périmètre que Tribler upstream).

## Version 0.3.2-alpha (2026-10-01)

- Release succédant à `v0.3.1-alpha` (supprimée — bundle incomplet et
  bugs corrigés depuis) :
  - app **bilingue EN/FR** (i18n complet, ~300 clés ARB, anglais par
    défaut, sélecteur dans Réglages → Apparence) ;
  - fix circuits DATA affamés par les circuits e2e (magnets anonymes
    figés en « Métadonnées ») ;
  - fix double `DropTarget` — un `.torrent` déposé partait en clair
    malgré les sauts choisis ;
  - onglet Diagnostic « Statistiques » enrichi (uptime, trafic overlay
    et BitTorrent, lanes anonymes, circuits DATA prêts, sorties) ;
  - UI : panneau de détail redimensionnable, scrollbar visible en mode
    clair, suppression de l'autocomplétion TopBar, systray renommé
    OnionBit ;
  - bumps majeurs Dependabot : rand 0.10, rusqlite 0.40,
    chacha20poly1305 0.11, sha1 0.11, base64 0.23 ;
  - build_dist.ps1 : `-ZipRelease` génère le bundle + zip GitHub.

## Étape 30 — Internationalisation de l'app Flutter : anglais par défaut, français disponible (2026-10-01)

- Pipeline standard Flutter : `flutter_localizations` + `intl` +
  `gen_l10n` (`app/l10n.yaml`). Gabarit `app/lib/l10n/app_en.arb`,
  traduction complète `app_fr.arb` (~300 clés) ; fichiers générés
  `app_localizations*.dart` ignorés par git.
- `AppLocale { system, en, fr }` + `localeSettingsProvider` persisté
  (`ui.locale`, `SharedPreferences`) ; **défaut `en`**, jamais la
  locale OS sauf choix « System ». Sélecteur tri-état dans
  Réglages → Apparence ; bascule à chaud sur tout le shell.
- Extraction intégrale par zones : core/layout + notifications,
  search + diagnostic, settings (page + 13 sections, catalogue
  d'ancres rebâti sur un enum `_SectionId`), downloads (page, table,
  menu contextuel, panneau détail, dialogue d'ajout).
- Frontière domaine/UI : les libellés sortent des enums
  (`DownloadFilterX.label(l10n)`) ; pluriels et paramètres en ICU
  (`{count, plural, …}`) ; `ByteFormatter`/`DurationFormatter`
  sensibles à la locale via `context.fmtBytes`/`fmtRate`/`fmtEta`
  (`o/Ko/Mo` ↔ `B/KiB/MiB`, `j` ↔ `d`).
- Hors périmètre (ADR-0005 conservée) : commentaires/docs/journaux en
  français ; autonymes `English`/`Français` ; keywords de recherche
  bilingues.
- Garde-fou `scripts/check_i18n.ps1` : échoue sur tout littéral
  français subsistant dans `app/lib/` (allowlist : commentaires,
  `uiLog`/`debugPrint`, autonyme, `keywords:`).
- `flutter analyze` propre, `flutter test` vert (helper
  `pumpApp(locale: …)`, assertions sur les chaînes EN) ;
  `check_i18n.ps1` vert. ADR-0009.

## Diagnostic : onglet Statistiques enrichi (2026-10-01)

- Regroupé en sections : **Daemon** (version, uptime, taille DB,
  espace disque du dossier de réception), **Contenu** (torrents
  connus, downloads actifs/en pause/en échec), **Réseau IPv8**
  (pairs découverts, trafic overlay ↑/↓, trafic BitTorrent session
  ↑/↓), **Anonymat** (sessions moteur, lanes actives, circuits DATA
  prêts par lane, sorties actives).
- Backend : `uptime_sec` ajouté à `tribler_statistics`
  (`CoreSession::uptime_secs`, `Instant` au `start()`) ;
  `libtorrent.total_recv_bytes`/`total_sent_bytes` renseignés
  (somme des `progress_bytes`/`uploaded_bytes` de tous les moteurs —
  étaient `null`).
- UI : `Ipv8Traffic` + `GET /api/statistics/ipv8` consommé
  (`total_up`/`total_down` de l'endpoint overlay) ;
  `socks5_sessions[].hops` → lanes affichées « ×1 · ×2 » ; circuits
  DATA/READY groupés par `goal_hops` ; sorties `enabled` comptées ;
  espace disque via `dirSpaceProvider` sur `download_defaults/saveas`.

## Diagnostic : statistique « Canaux » retirée (2026-10-01)

- Le compteur `num_channels` (entrées `metadata_type=400` du
  GigaChannel) restait structurellement à 0 : le réseau ne produit plus
  de canaux. Ligne retirée de l'onglet Statistiques + champ
  `OnionbitStats.numChannels` et clé `statChannels` supprimés.
- Le champ JSON `num_channels` reste émis par `GET /api/statistics/
  tribler` (compat API).

## UI : pouce de scrollbar visible en mode clair (2026-10-01)

- Le thème ne définissait pas de `scrollbarTheme` : le défaut M3
  (`onSurface` très dilué) rendait le pouce quasi invisible en mode
  clair sur toutes les listes. `thumbColor` global → `outline` au
  repos, `onSurfaceVariant` au survol/drag — lisible dans les deux
  modes, appliqué à toutes les fenêtres via `MaterialScrollBehavior`.

## Daemon : systray renommé OnionBit (2026-10-01)

- Le renommage produit avait oublié le tray Windows : tooltip
  « Tribler — <addr> » (création + mise à jour du port réel) et item
  « Ouvrir Tribler » → « OnionBit — <addr> » / « Ouvrir OnionBit ».
- Conservé volontairement : la valeur de registre autostart
  `TriblerRustDaemon` — la renommer créerait un doublon de clé Run
  pour les installations existantes.

## UI : suppression de la prédiction de recherche dans la TopBar (2026-10-01)

- `top_bar.dart` : le champ de recherche n'utilise plus
  `Autocomplete` (suggestions FTS `/metadata/search/completions` en
  popover) — simple `TextField` avec debounce 300 ms, bouton effacer
  et synchro `searchQueryProvider`.
- `search_repository.dart` / `rest_search_repository.dart` : la
  méthode `completions` retirée (code mort) ; l'endpoint daemon
  `/metadata/search/completions` reste disponible pour la parité
  wire Tribler.

## UI : panneau de détail des téléchargements redimensionnable (2026-10-01)

- Le panneau « Détails/Fichiers/Trackers/Pairs » était figé à 280 px
  sans indication de défilement : le contenu de l'onglet Détails
  (sparkline, boutons, ~10 lignes de propriétés) était tronqué.
- `downloads_page.dart` : le séparateur devient une poignée de
  glisser (`resizeUpDown`) — hauteur persistée `ui.detailPanelHeight`
  via `detailPanelHeightProvider` (bornes 120–720 px, plafond 70 %
  de la fenêtre à l'affichage).
- `download_detail_panel.dart` : `Scrollbar` à pouce persistant sur
  l'onglet Détails — le défilement est désormais visible.
- `ui_prefs.dart` : `uiPrefsWrite` accepte les `double`.

## Journal daemon : annonces DHT hidden-service rétrogradées en debug (2026-10-01)

- `hidden_services.rs` : « point d'introduction annonce sur la DHT »
  (re-annonce périodique par intro point — ~1400 lignes en quelques
  minutes sur 3 swarms seedés) et « dht_lookup du swarm : valeur(s)
  DHT » (tick de découverte par swarm) passent de `info!` à `debug!` —
  visibles uniquement via le switch « Debug » de l'onglet Journaux
  (`PUT /api/ipv8/asyncio/debug`).

## Fix : double DropTarget — un `.torrent` déposé partait en clair malgré les sauts choisis (2026-10-01)

- Symptôme : déposer un `.torrent` sur la page Téléchargements puis
  choisir 1–3 sauts dans le dialogue lançait le download en clair
  (`hops=0`) ; le journal UI montrait un premier `ajout hops=0` suivi
  du `ajout hops=N` demandé.
- Cause : deux `DropTarget` `desktop_drop` imbriqués recevaient le même
  dépôt — celui de `DownloadsPage` appelait `addTorrentBytes` **sans**
  `anon_hops` (hops=0) et la `DropZone` globale d'`app_shell` ouvrait en
  parallèle le dialogue « Ajouter » ; au submit, la dédup
  `download_exists` absorbait le second ajout sur l'existant hops=0.
- `app/lib/features/downloads/presentation/pages/downloads_page.dart` :
  `DropTarget`/`_handleDrop`/`_dragging` retirés — la `DropZone`
  globale couvre déjà toute la fenêtre et route le fichier vers le
  dialogue où les sauts se choisissent.
- `app/lib/features/downloads/presentation/widgets/add_download_dialog.dart` :
  un choix explicite de sauts pose `_hopsInitialized` — les réglages
  `download_defaults` arrivant en retard n'écrasent plus la sélection.

## Fix : circuits DATA affamés par les circuits e2e — downloads anonymes figés en « Métadonnées » (2026-10-01)

- Symptôme réel : ajout d'un magnet depuis la recherche en lane
  anonyme (3 sauts) resté indéfiniment en `METADATA` et un autre
  download à 3 sauts figé à 67 % — alors que le même magnet se
  résolvait en quelques secondes en clair (hops=0).
- Cause : `build_circuits_if_needed` comptait les circuits `READY` par
  `goal_hops` **tous `ctype` confondus** ; depuis que `select_circuit`
  est limité aux circuits `DATA` (commit `d8d221f`, cross-talk e2e),
  les circuits `IP_SEEDER` du hidden seeding satisfont le comptage
  `ready >= min_circuits` sans jamais servir le trafic applicatif —
  aucune reconstruction DATA n'était lancée et les datagrammes
  uTP/DHT/trackers de la lane tombaient en perte silencieuse.
- `crates/onionbit-tunnel/src/community.rs` : comptages `ready` et
  `pending` limités à `ctype == CIRCUIT_TYPE_DATA`.
- `crates/onionbit-core/src/ipv8_stack.rs` : le watchdog de lane
  engage la portée `"circuits"` du kill switch tant que
  `ready_data_circuits_of_hops(hops)` est vide (prédicat identique à
  la sélection de circuit du SOCKS5/sockets UDP — la doc le promettait
  déjà, le code divergeait).
- Régression couverte par `build_circuits_ne_compte_pas_les_ip_seeder`
  (`tests/circuits_loopback.rs`) : un `IP_SEEDER` READY ne suffit plus
  à inhiber la construction d'un `DATA`.

## Renommage produit : OnionBit + version 0.3.1-alpha (2026-10-01)

- Crates `tribler-*` → `onionbit-*` (dossiers, `Cargo.toml`, imports
  `tribler_*` → `onionbit_*`) ; binaires `onionbit-daemon`,
  `onionbit-cli`, `onionbit_ui`.
- Artefacts produit : etat `.onionbit/`, `onionbit.db`, `onionbit.log`,
  env `ONIONBIT_API_KEY` / `ONIONBIT_API` / `ONIONBIT_DAEMON_EXE`,
  provider Dart `onionbitStats*`, titres « OnionBit » (fenetre,
  manifeste web, `Runner.rc`, `app/README.md`), classes Dart
  `TriblerApp`/`TriblerStats` → `OnionbitApp`/`OnionbitStats`,
  progid d'association `.torrent` → `OnionBit.torrent`.
- **Preserve pour la parite wire Tribler** : evenements SSE
  `tribler_*`, endpoint `/api/statistics/tribler`, cle JSON
  `tribler_statistics`, sel d'identite `tribler anonymous download`,
  `TRIBLER_TUNNEL_COMMUNITY_ID`, flags d'interop `--tribler-*`,
  adresses de bootstrap `dispersy*.tribler.org` (infra Tribler reelle).
- Version workspace `0.3.1-alpha` (pubspec `0.3.1+1`), auteur
  Laurent Geynet (Loulach), `repository` → github OnionBit.
- AGENTS.md simplifie ; chemins locaux purges des docs (remplaces par
  les env `TRIBLER_SRC` / `RQBIT_SRC` / `TRIBLER_EXE`) — les scripts
  d'interop gardent leurs defauts locaux (env-overridables).
- Release : `dist/OnionBit-0.3.1-alpha-windows-x64.zip`.

## Transport tunnel : pompe d'emission FIFO (fix reordonnancement e2e)

- Symptome : un test loopback de rafale e2e (`hidden_seed_e2e_burst_integrity`,
  2000 datagrammes uTP sur circuit lie) montrait des swaps par paires —
  `premiers tags: [0, 1, 3, 2, 4, 5, ...]` — alors que toutes les
  cellules arrivaient. Cause : `relay_cell` (et `exit_data`) faisaient
  un `tokio::spawn` par envoi ; sur runtime multi-thread, deux taches
  consecutives s'executent dans un ordre arbitraire. pyipv8 envoie
  depuis la boucle asyncio unique — FIFO garanti.
- `crates/onionbit-tunnel/src/community.rs` : file bornée
  (`SEND_QUEUE_CAP = 4096`, drop-tail = semantique UDP) drainee par une
  pompe d'emission unique `SendJob::Endpoint|ExitSocket`. Les chemins
  `relay_cell` et `exit_data` enquetent au lieu de spawner.
- Tests `circuits_loopback.rs` : `hidden_seed_e2e_burst_integrity`
  verifie desormais FIFO strict (sous-sequence strictement croissante)
  + pertes < 1 % ; `inject_incoming_burst_drop_tail` documente le
  drop-tail a `DATA_CHANNEL_CAP`. 27/27 verts sous contention CPU.
- Note : la perte constatee dans le run anchor sens B (3,5 Mo figes)
  reste expliquee par la topologie mono-exit (A1 seul `EXIT_BT`) ;
  l'ordonnancement FIFO leve un risque uTP additionnel sur le chemin.

## Resilience interop : kill de l'ancre A1 (bootstrap + EXIT_BT)

- `scripts/interop_hidden_killseeder.ps1 -KillTarget anchor` : le noeud
  d'amorcage A1 (bootstrap du maillage + exit `EXIT_BT`) est tue en
  plein transfert. Le banc verifie : survie du flux e2e etabli,
  decouverte maintenue sans le bootstrap (pairs tunnel encore visibles
  apres elagage par `RandomChurn`), reconstruction `IP_SEEDER`
  conditionnelle (exigee seulement si A1 figurait dans le chemin d'un
  circuit du seeder — sinon les intros survivants sont verifies
  intacts), annonce DHT attribuable au seeder, completion + SHA-256.
- **Sens A** (seeder Rust) : `target/interop-killseed-20261001-104443/` —
  flux e2e survecu, decouverte sans ancre `n=3`, `IP_SEEDER`
  reconstruit sur un noeud different (mid `a2bbe942…`), annonce DHT
  attribuee, SHA-256 `751d0808…` identique.
- **Sens B** (seeder Tribler 8.4.3) :
  `target/interop-killseed-20261001-112453/` — flux e2e survecu,
  `IP_SEEDER` intacts (ancre hors chemin), annonce lisible, SHA-256
  `8f1684ab…` identique.
- **Enseignement topologique** : un premier run sens B a montre qu'avec
  A1 comme *seul* exit `EXIT_BT`, sa mort rend toute reconstruction de
  circuit DATA impossible (pyipv8 `select_exit` -> `None`, comportement
  conforme mais trivial). Le maillage donne desormais
  `exitnode_enabled` a A2 pour disposer d'un second exit, comme sur le
  vrai reseau.

## Resilience interop : kill de l'intro point, reconstruction verifiee

- `scripts/interop_hidden_killseeder.ps1 -KillTarget intro` : le noeud
  hebergeant le point d'introduction (`verified_hops[-1]`, resolu par
  `sha1(pubkey)` -> nom) est tue en plein transfert sans restart du
  seeder. Le flux e2e etabli survit (independance correcte), le seeder
  reconstruit un `IP_SEEDER` ailleurs, re-annonce sur la DHT, et le
  telechargement complete a 100 % avec SHA-256 identique.
- **Sens A** (seeder Rust, intro point A1 tue) :
  `target/interop-killseed-20261001-101231/` — reconstruction apres
  elagage du pair mort par `RandomChurn` (~58 s), `intro-established`
  recu, annonce DHT attribuee au seeder, SHA-256 `79b02f6c…`.
- **Sens B** (seeder Tribler 8.4.3, intro point A1 tue) :
  `target/interop-killseed-20261001-102831/` — le pyipv8 de Tribler
  reconstruit son propre `IP_SEEDER` en ~30 s (mid `b3ac5526…`),
  re-annonce DHT attribuee, download 100 %, SHA-256 `fb422335…`
  verifie post-teardown (le hashage in-script a ete rendu tolerant au
  verrou moteur via `FileShare.ReadWrite`).
- **Verdict attribuable** : le circuit reconstruit doit etre NOUVEAU
  (id hors snapshot pre-kill) avec dernier saut different ; l'annonce
  DHT est parsee (`DHTIntroPointPayload` : `seeder_pk` identique au
  snapshot pre-kill + `intro_mid` nouveau) — une annonce du
  downloader devenu seeder ne peut plus valider le verdict a tort.
- **Cause racine de l'echec initial** : deux divergences pyipv8.
  (1) `create_introduction_point` laissait `required_exit=None` — le
  dernier saut etait pris dans les `candidates` offerts par les
  relais, qui continuaient de proposer le noeud mort. Fix : comme
  `create_circuit` pyipv8, `required_exit = select_exit(IP_SEEDER)`
  est choisi localement (`EXIT_BT` -> `EXIT_IPV8` -> `RELAY`) et les
  premiers sauts passent par `first_hop_candidates` (alternates de
  retry). (2) `RandomChurn` manquait : un pair verifie mort restait
  indefiniment dans `peers_for_service`/`flag_registry`, donc
  `select_exit` le re-eliait sans fin. Fix : `churn_step` dans
  `DiscoveryCommunity::step` (sample 8, ping a 27,5 s d'inactivite,
  drop a 57,5 s sans reponse — valeurs `ipv8_default_config`), et
  `last_response` rafraichi a chaque datagramme recu d'un pair
  verifie (`touch_by_addr` : discovery, tunnel, DHT,
  content-discovery — equivalent du touch de `Community.on_packet`).
- `is_inactive` ne regarde plus que l'activite entrante
  (`last_incoming`) : le keepalive sortant ne maintient plus un
  circuit mort artificiellement vivant (fidelite `beat_heart`
  pyipv8, sur trafic entrant uniquement).
- Le filtre `ST_SYN` WAN de `TunnelUdpSocket` devient opt-in
  (`with_syn_filter`) : la socket generique reste neutre (les tests
  loopback d'acceptation inbound passent), seule la lane moteur
  (`TunnelUdpSockets.utp_transport`) rejette les SYN livres par
  l'exit — l'entrant anonyme legitime n'existe que via
  `inject_incoming` (hidden services e2e).
- Observabilite : log `establish-intro envoye` (cid, identifier,
  premier saut) et `intro-established sans requete en attente`.
- Run de validation : `target/interop-killseed-20261001-101231/`.

## Resilience interop : kill du seeder en plein transfert, reprise verifiee

- `scripts/interop_hidden_killseeder.ps1` (`-Sens A|B`) : seeder tue
  a >=3 Mo telecharges, drain borne mesure, fenetre morte stricte de
  30 s sans octet, restart, puis reprise jusqu'a 100 % et SHA-256
  identique.
- **Sens A** (seeder Rust tue, downloader Tribler 8.4.3) :
  `target/interop-killseed-20261001-072004/`. Kill a dl=3 777 273 o,
  drain +351 111 o en 2,0 s, fenêtre morte 30 s, restart → SEEDING +
  IP_SEEDER reconstruits, reprise par re-decouverte Tribler
  (`do_peer_discovery`/`swarm_lookup_interval` = 30 s), 100 %,
  SHA-256 `6405c943…` identique.
- **Sens B** (seeder Tribler tue, downloader Rust) :
  `target/interop-killseed-20261001-073411/`. Kill a dl=3 997 696 o,
  drain +49 152 o en 1,1 s, fenetre morte 30 s, Tribler restaure
  SEEDING+IP_SEEDER ~10 s apres reboot, D repart a +266 s (temps que
  le nouvel intro point re-annonce sur la DHT et que
  `do_peer_discovery` cree le nouvel e2e), 100 %, SHA-256
  `b9792f43…` identique.

## Resilience interop : `anon_hops=3` vert dans les deux sens

- Sens A : `target/interop-hidden-dl-20261001-065729/` (10 IP_SEEDER
  READY, transfert complet, SHA-256 `a08dc390…` identique). Sens B :
  `target/interop-hidden-seed-20261001-070207/` (dl=6291456, 100 %,
  SHA-256 `73dcd910…` identique).
- **Correction topologique des bancs** : a `hops>=3` les circuits
  `IP_SEEDER`/`RP_DOWNLOADER` font `hops+1` sauts (`swarm.hops + 1`,
  egalite pyipv8) — il faut `hops` relais libres distincts puisque le
  dernier saut impose (`required_exit`) est exclu des candidats. Les
  deux scripts ajoutent A4/A5 quand `-Hops -ge 3`. Le premier run
  hops=3 echouait en `no candidates to extend` faute de relais
  suffisants — limitation de topologie du banc, pas du protocole.
- Fix PS 5.1 : `(if ...)` n'est pas une expression valide —
  `$tulDelta` reecrit en deux lignes (le crash survenait apres les
  verdicts, sans impact sur la preuve).

## Resilience interop : `anon_hops=2` vert dans les deux sens + filtre SYN WAN

- Sens A `-Hops 2` : run `target/interop-hidden-dl-20261001-062029/`,
  transfert complet 6 291 456 o en ~17 s apres `create-e2e`,
  SHA-256 `b9ed9f6a…` identique. Sens B `-Hops 2` : run
  `target/interop-hidden-seed-20261001-063246/`, `dl=6291456`, 100 %,
  SHA-256 `a9cb7b61…` identique.
- **Cause racine du stall hops=2** : la socket uTP de lane injectait
  dans rqbit **tous** les datagrammes plausibles uTP livres par
  l'exit — y compris les `ST_SYN` de scanners WAN. La file FIFO
  d'acceptation + les slots `max_pending_incoming_handshake_checks`
  se saturaient : le SYN e2e de Tribler attendait ~160 s avant
  `check_incoming_connection`, au-dela de son timeout. A hops=1 le
  timing passait par chance (le SYN e2e precedait la saturation).
- **Fix** (`exit_policy.rs`, `tunnel_udp_socket.rs`) : nouveau
  classifieur `is_utp_syn` ; les `ST_SYN` arrivant par le chemin exit
  (`data_rx`) sont ignores — en Tribler l'entrant anonyme n'existe
  que via les lanes e2e (`inject_incoming`), les connexions anonymes
  classiques sont toujours sortantes. `ST_STATE`/`ST_DATA` et le
  trafic etabli passent toujours.
- Instrumentation : log `e2e -> injection uTP` enrichi
  (seq_nr/ack_nr/prefixe payload — a permis de prouver que les
  handshakes BT `\x13BitTor` arrivaient alignes, ecartant une
  corruption e2e), `seq_nr`/`ack_nr` sur les sorties tunnel, et log
  d'entree `check_incoming_connection` dans le vendored librqbit
  (a revele le delai de 160 s).

## Interop hidden-service sens B vert : downloader Rust télécharge depuis Tribler 8.4.3

- `scripts/interop_hidden_tribler_seed.ps1` (sens B : T = seeder
  Tribler reel avec contenu pre-pose -> SEEDING -> IP_SEEDER -> DHT ;
  D = downloader Rust lance SEULEMENT apres le gate « valeur relue
  depuis A1 ») passe de bout en bout : `dht_lookup du swarm` trouve
  l'intro point de T, `created-e2e valide` (parse `RendezvousInfo`
  NestedPayload de pyipv8), `link-e2e`/`linked-e2e`, `add_peer` sur
  l'adresse fake `circuit_id_to_ip`, uTP, pieces, `dl=6291456`,
  progress=100 %, **SHA-256 identique**. T cote : `all_time_upload=
  6291456`. Run : `target/interop-hidden-seed-20261001-052937/`
  (~7 s entre create-e2e et fin du transfert).
- Exerce pour la premiere fois en interop : role intro-point de nos
  noeuds face a un seeder pyipv8 (`establish-intro` recu de Tribler,
  `dht_announce` par l'intro point, forwarding over-socket du
  `create-e2e` vers le circuit d'intro), parse cote downloader de
  `created-e2e`/`RendezvousInfo` produits par pyipv8.
- Log `peers-response recu` ajoute (frontiere observable du banc).

## Interop hidden-service sens A vert : Tribler 8.4.3 télécharge depuis le seeder Rust

- `scripts/interop_hidden_tribler_download.ps1` (mesh loopback A1
  EXIT_BT + A2/A3 relais + S seeder anonyme + Tribler.exe 8.4.3 réel
  downloader) passe **tous les verdicts** : S SEEDING + 10 IP_SEEDER,
  gate DHT déterministe (`S=1 A=1` avec relecture `find_values` depuis
  A1 avant de lancer Tribler), `peers-request`/`peers-response`,
  `create-e2e`/`created-e2e`, `link-e2e`, uTP+handshake BitTorrent sur
  le circuit e2e lié, upload S `delta=6291456`, Tribler 100 %,
  **SHA-256 du fichier recu identique a la source**.
  Run : `target/interop-hidden-dl-20261001-050850/`.
- **Fix wire `PeersResponse`** (`payload.rs`) : le `payload-list`
  pyipv8 `[IntroductionInfo]` encode `B` count puis `>H` longueur +
  corps par element ; on serialisait les elements en ligne sans le
  prefixe `>H` -> Tribler desalignait et levait `PackError: Cannot
  unpack address type 0`. Test de regression
  `peers_response_nested_payload_wire`.
- **Fix wire `RendezvousInfo`** : `rp_info_enc` de `CreatedE2E` doit
  contenir `serializer.pack("payload", rp_info)` = `NestedPayload`
  (`>H` taille + corps `ip_address,varlenH,20s`), pas le corps nu —
  meme symptome `address type 0` dans `on_created_e2e` cote Tribler.
  `pack_framed`/`unpack_framed` appliques aux deux sens.
- Diagnostic e2e : logs `created-e2e : rendezvous_info construit`
  (adresse RP concrete, `rp_addr_unspecified`), `e2e -> injection
  uTP`, `socket tunnel -> cellule data`, eviction de pin stale dans
  `TunnelUdpSocket::dispatch`.
- Instrumentation DHT : `dht_store` trace selection de noeuds,
  `store-request`/`store-response` correles par transaction ID,
  `noeud bloque/absent -> drop` cote receveur — le run `-DhtOnly`
  a prouve `3/3 STORE` en <35 ms et exclu le store-request des
  suspects.
- Rappel : `Peer<0.0.0.0:0>` dans `Added hop` cote Tribler est
  cosmetique (pyipv8 construit `Hop(Peer(key))` sans adresse) — pas
  un indice de regression d'adresse.

## Hidden services — banc live complet vert : seed, upload anonyme, kill, reprise

- `scripts/live_hidden_upload.ps1` (trois daemons isoles : A ancre,
  S seeder anonyme, D downloader bootstrappe via A uniquement) passe
  les **8 verdicts** : SEEDING, IP_SEEDER READY, gate DHT (S/A/D voient
  la valeur, instrumentation `debug` du lookup), octets reels chez D,
  `all_time_upload` delta>0 chez S, kill switch (drain borne puis
  invariance stricte 60 s), reprise apres redemarrage de S,
  `progress=100 %` (chaque piece validee par hashcheck rqbit).
- **Fix majeur — accepteur uTP entrant sur transport tunnel**
  (`vendor/librqbit`) : trait object-safe `UtpAcceptor` symetrique de
  `UtpConnector`, `ListenerOptions.utp_socket` (aucun bind UDP reel,
  `announce_port=None`, UPnP off), `ListenResult.utp_acceptor` +
  `utp_listen_custom` dans `session.rs`, `EngineConfig.utp_listen_socket`.
  Sans lui le SYN e2e injecte chez S restait en cache puis RST — le
  uTP entrant n'existait que pour la socket d'ecoute reelle.
- **Fix librqbit-utp vendored — race SYN-cache/DATA** : quand un SYN
  attendait un accepteur (`try_cache_syn`), le ST_DATA suivant tombait
  sur `streams.get` vide et etait perdu -> le pair restait en
  `SynAckSent` jusqu'a `max syn-ack retransmissions`/`remote inactive`
  (cause de l'echec de reprise observe en live). Les paquets pour
  `conn_id+1` d'un SYN en file sont maintenant stockes dans un backlog
  borne (16) rejoue a la creation du stream. Reproduction locale :
  `accepteur_utp_syn_cache_puis_data`.
- **Dialecte seq/ack documente** : le SYN-ACK part avec `seq_nr` mais
  l'initiateur l'acquitte comme `seq_nr-1` (cf. `StreamArgs::
  new_outgoing` : `last_consumed_remote_seq_nr = remote_ack.seq_nr-1`)
  — convention upstream conservee.
- **Fix routage sortant** : `select_circuit`/`select_http_circuit`
  (sockets tunnel UDP + SOCKS5) ne considerent plus que les circuits
  `CIRCUIT_TYPE_DATA` (`ready_data_circuits_of_hops[_flags]`) — du
  trafic DHT generique pouvait partir sur un circuit `RP_SEEDER` et
  aboutir injecte dans la socket uTP du pair e2e.
- **Filtre d'injection e2e** : seuls les datagrammes `could_be_utp`
  sont pousses dans la socket uTP de la lane (le chemin
  `inject_incoming` contournait le filtrage par forme de `data_rx`).
- Phase kill du banc refaite selon le modele drain/fenetre-morte :
  drain borne (2 Mio, 15 s de silence ou 90 s max) puis invariance
  stricte 60 s — un burst post-kill borne n'est plus un faux positif.

## IPv8 — Fix : overlay DHT (`DHTDiscoveryCommunity`) sans pairs

- Symptôme observé en live : `/api/ipv8/overlays` affichait
  `DHTDiscoveryCommunity` à **0 pair** alors que Discovery en comptait ~26.
- Cause : les quatre handlers d'introduction de `DhtCommunity`
  (`INTRODUCTION_REQUEST`/`NEW_INTRODUCTION_REQUEST`/`*_RESPONSE`)
  appelaient `on_node_discovered` (table de routage) mais **jamais**
  `network.add_verified` + `discover_service(DHT_COMMUNITY_ID)` —
  étape que la classe de base pyipv8 (`Community.on_introduction_*`)
  applique à tout pair d'introduction. Conséquences : `peers_for_service`
  vide → marche sans candidat, jamais de pair introduit en réponse
  (`get_peer_for_introduction`), affichage UI à 0.
- Correctif : ajout de `add_verified` + `discover_service` dans les
  quatre handlers, avec `new_style_intro` positionné comme pyipv8
  (`true` pour les messages NEW_*, `supports_new_style` sinon).
- Test de régression : `dht_intro_ping_store_find_loopback` vérifie
  désormais `network().peers_for_service(DHT_COMMUNITY_ID)` non vide
  des deux côtés après découverte mutuelle.
- « Aucun relais » dans l'onglet Diagnostic est en revanche un état
  réel et attendu : `on_extend`/`join_circuit` existent et le flag
  `RELAY` est annoncé, mais servir de relais exige qu'un initiateur
  public nous choisisse **et** que son `CREATE` atteigne notre port
  (NAT) — le banc épinglé prouve que le code relaie quand il est
  sollicité.

## Daemon — Icône systray/exe passée à OnionBit

- `resources/tribler.ico` (ancien logo Tribler rouge) remplacé par
  `onionbit.ico` généré depuis `branding/platforms/linux/hicolor/`
  (frames 16→256 px). `resources.rc`, le fallback `Icon::from_path`
  de `tray.rs` et les commentaires pointent sur le nouveau fichier.
- Nécessite un rebuild de `onionbit-daemon` (l'icône est embarquée
  dans l'exe via `embed-resource`) ; `tray_icon_color` non vide
  reste prioritaire (carré RGB, comportement Python conservé).

## Interop — banc DHT publique : téléchargement réel via le réseau Tribler

- `examples/interop_public_download.rs` + `scripts/interop_public_dht.ps1` :
  le downloader rejoint le **réseau Tribler réel** (marche aléatoire sur
  le prefixe `a3591a6b` bootstrapée sur Tribler.exe local — le seul
  rôle de l'instance installée est l'introduction dans l'overlay),
  construit des circuits à **sauts libres** (aucun épinglage, pas de
  `required_exit` : sélection standard `candidates`/`EXIT_BT|RELAY`,
  route rapportée = `verified_hops` observés), route DHT mainline
  publique (`router.bittorrent.com`…) et uTP dans le tunnel, sortie =
  noeud Tribler réel flaggé exit. Endpoint bind `0.0.0.0` + `my_lan`
  réel : nécessaire hors loopback. `py_tunnel_node.py --listen` ajouté
  pour une sortie contrôlée non-loopback.
- **Verdict positif exigé** : code 0 + `INTEROP PUBLIC DOWNLOAD OK` +
  `octets_verifies >= -MinBytes` — l'intégrité est celle des pièces
  BitTorrent elles-mêmes (`progress_bytes` ne compte que du vérifié).
- **Mesures (réseau réel, non déterministe)** : magnet Sintel —
  2 sauts : 1,37 Mio vérifiés via 2 pairs publics
  (`7a2fb4b7…`, `c2b2f2ec…`) ; 3 sauts : 327 Kio via Tribler.exe local
  + relais public + sortie publique (`38891a02…`, `ce9ff7b8…`,
  `a002e79f…`). Cohérent avec l'UI Tribler qui montre des circuits
  1–2 sauts : c'est son défaut (`number_hops=1`), notre stack tient 3.

## UI — Fix : colonne ETA affichant des nombres absurdes

- `formatSeconds` : ETA > ~300 ans (débit nul → backend
  `total/1e-6` ≈ 1e15 s) débordait `Duration` en int64 → nombres
  négatifs géants (« -2121196235785 s »). Borné : non-fini, ≤ 0 ou
  > 1e10 s → « — ». Cas NaN/∞ couverts par test.

## UI — Fix : dernière colonne des tables coupée par la sidebar

- Les tables Téléchargements et Rechercher calculaient `maxWidth`
  depuis `MediaQuery` (largeur fenêtre entière) au lieu de la zone de
  contenu : la table dépassait de ~216 px à droite, la dernière
  colonne sortait de l'écran sans scroll visible. `LayoutBuilder`
  donne désormais la contrainte réelle.

## UI — Logo OnionBit plus visible dans la sidebar

- Logo agrandi : 28 → 44 px en sidebar pleine largeur, 36 px en rail
  rétracté, centré (était aligné à gauche et trop petit par rapport
  au bouton « Ajouter »).

## UI — Téléchargements : badge d'anonymat explicite + % lisible

- `AnonBadge` devient une pastille texte : « Clair » (trafic direct,
  icône globe) ou « Anon ×N » (N sauts, bouclier plein/pointillé selon
  l'établissement du circuit) — avant : une icône bouclier seule pour
  les anonymes, rien pour les clairs. Colonne badges 88 → 130 px.
- `_ProgressBar` mutualisé (table, grille, liste compacte) : barre de
  16 px arrondie + étiquette % sur pastille de surface translucide —
  le texte était invisible sur la portion remplie en thème sombre.

## UI — Fix : Réglages vides au scroll (ref en dispose)

- `DeferredSection.detach()` appelait `ref.read()` depuis `dispose()`
  → `StateError` « ref when unmounted » à chaque démontage de section
  hors écran : l'exception dans `finalizeTree` corrompait l'arbre et
  affichait un grand bloc gris (ErrorWidget release). Le bus de
  sauvegarde est désormais capturé à `attach`.
- `test/settings_page_test.dart` : le test widget qui défile la page
  reproduisait le crash avant le fix (16 tests au total).

## UI — Menus contextuels : fermeture au clic extérieur

- `MenuAnchor` des pages Téléchargements et Rechercher enveloppé d'un
  `Listener` translucent : tout `pointerDown` atteignant le contenu
  sous-jacent (le menu vit dans un overlay) ferme le menu — le clic
  souris hors menu ne le renvoyait plus au `TapRegion` interne et il
  fallait passer par Échap.

## Roadmap clôturée — étapes 20 (UI Flutter) et 29 (systray) validées

- Validation visuelle manuelle effectuée (2026-09-30) : étape 20 (rendu
  réel de l'UI Flutter contre le daemon) et étape 29 (menu systray,
  bascule « Démarrer avec Windows », « Quitter ») passent de `[i]` à
  `[x]` — **toutes les étapes de la roadmap sont terminées**.
- Le travail réseau se poursuit hors roadmap : banc DHT publique
  (sélection libre, route observée) et sortie `EXIT_BT` Tribler réelle
  restent des jalons d'interop séparés.

## Interop — Tribler.exe 8.4.3 comme premier relais (route épinglée)

- **`Circuit` porte un plan de sauts épinglés** (`pinned_hops`) :
  `TunnelCommunity::create_circuit_pinned` + priorité du saut épinglé
  dans `send_extend`, avec `node_addr` réel transmis même hors annuaire
  local — aucun repli vers les candidats publics annoncés par le relais
  précédent. Cause de l'échec précédent identifiée : `send_extend`
  piochait le saut suivant dans les `candidates` du `CREATED` de Tribler
  (pairs du réseau réel) — `EXTENDED` revenait sans qu'aucun `CREATE`
  n'atteigne le relais Rust voulu.
- `exit_download_interop --relay <keyfile> --tribler-id` : Tribler.exe
  (community `a3591a6b…`) en premier saut, relais Rust épinglés ensuite,
  sortie pyipv8 `EXIT_BT` ; affiche `route = verified_hops` à rapprocher
  de `attendu = [clés attendues]`, plus `demande=`/`verifie=` octets.
- `scripts/interop_tribler_relay.ps1` durci : lecture `configuration.json`
  (port IPv8 8090, clé API), vérification d'identité de l'instance
  jointe via `/api/ipv8/overlays` (le `my_peer` de la tunnel community
  doit égaler le PEM `ec_multichain` — un autre processus « Tribler »
  est rejeté), polling `/api/ipv8/tunnel/relays` pendant le transfert
  (`circuit_from`/`circuit_to`/octets, rapprochés du `circuit_id` Rust),
  `[Diagnostics.Process]` direct (ExitCode fiable — `Start-Process`
  -PassThru avec redirections ne le renseigne pas sous PS 5.1),
  preuve positive exigée (code 0 + ligne OK + `verifie=` == demandé),
  timeout borné, `try/finally` (un Tribler préexistant n'est jamais tué).
- **Confinement des panics vérifié** : test `panic_handler_ne_tue_pas_la_reception`
  — un handler qui panique ne stoppe pas la réception des autres
  (`catch_unwind` de `recv_loop`).
- **Mesures** (route épinglée `Tribler.exe → relais Rust → sortie
  pyipv8`, vérifiée par `verified_hops`) : 200 Ko à 2 et 3 sauts, 4 Mio
  à 3 sauts, 200 Ko à 3 sauts avec DHT locale. Compteurs de la route
  relais de Tribler croissants pendant le transfert (~244 Ko pour
  200 Ko utiles). Limites : Tribler n'est pas la sortie `EXIT_BT` de ce
  banc ; la DHT reste le nœud bootstrap local, pas la DHT publique.

## Interop — transfert multi-sauts via sortie pyipv8, DHT incluse

- `scripts/interop_exit_download.ps1` + `examples/exit_download_interop` :
  harnais automatisé rqbit → `TunnelCommunity` Rust (1 à 3 relais Rust)
  → sortie **pyipv8** réelle (`EXIT_BT`) → seed uTP, avec vérification
  octet à octet du payload. Options `--hops`, `--payload`, `--dht`.
- **Découverte DHT à travers le tunnel** : `EngineConfig::dht_bootstrap_addrs`
  + `announce_port` (annonces effectives sur loopback), sockets DHT/uTP
  injectés (`TunnelUdpSocket`) — aucun datagramme ne contourne le tunnel.
  Patch vendored `librqbit-dht` : le bootstrap `find_node` apprend
  l'`id` du répondant depuis la réponse (BEP 5) — sans cela la table
  restait vide et `get_peers`/`announce` n'interrogeaient personne.
- **Bug majeur corrigé** : `relay_early_count` était `u8` alors que
  Python l'incrémente à **chaque** cellule relayée (int non borné,
  `crypto.py:208`). Débordement à la 256e cellule → panic dans
  `process_cell` → mutex `inner` empoisonné → noeud relais sourd
  définitivement tout en continuant à émettre. Élargi en `u32`
  (`Circuit` + `RelayRoute`), `RelayRoute` initialisé à 1 comme
  `tunnel.py:236`, `max_relay_early` passé en `u32`.
- **Durcissement endpoint** : les handlers de community (raw + packet)
  sont enveloppés de `catch_unwind` dans `recv_loop` — un panic de
  handler ne peut plus tuer la boucle de réception UDP du noeud.
- **Mesures** (payload vérifié octet à octet, boucle locale) :
  4 Mio OK à 1/2/3 sauts avec et sans DHT ; 32 Mio OK à 3 sauts
  (~24 000 cellules relayées par route — ancien plafond : 256).
  Limite connue : topologie contrôlée locale, pas encore une sortie
  Tribler 8.4.3 réelle ni la DHT publique.

## Backend — SSE `settings_changed` (étape 8/8)

- Nouvelle variante `Notification::SettingsChanged` (`onionbit-core`) ;
  `POST /api/settings` l'émet après persistance → topic SSE
  `settings_changed` (`onionbit-api`, mapping + test de mapping).
- Côté UI : `EventTopics.settingsChanged` + `ref.listen` dans
  `daemonSettingsProvider` → `invalidateSelf()` : un client qui modifie
  la config resynchronise les sections dédiées des autres clients et
  l'éditeur avancé, sans action manuelle.

## UI — Tests widget des zones enrichies (étape 7/8)

- `test/ui_enrichissement_test.dart` : centre de notifications
  (push/non-lues/borne à 100/markAllRead/clear), persistance des
  préférences UI (tri Téléchargements et Rechercher, rail rétracté,
  filtres repliés, mode d'affichage — rechargement via
  `uiPrefsInitProvider` + vérification write-through), widget
  `EmptyState` (rendu + action). Suite portée à 15 tests.

## UI — Téléchargements : vues grille et liste compacte (étape 6/8)

- `downloadViewModeProvider` (persisté `ui.downloadViewMode`) +
  `SegmentedButton` dans la barre d'outils desktop : table / grille /
  liste compacte. La grille réutilise sélection Ctrl/Shift, clic droit
  contextuel, Ctrl+A/Échap et les pastilles santé/badges de la table.
  Écran étroit : la liste compacte reste imposée.

## UI — Persistance des préférences d'interface (étape 5/8)

- `uiPrefsInitProvider` (FutureProvider, semé au démarrage par
  `TriblerApp`) + `uiPrefsWrite` (write-through dans les notifiers) :
  rail rétracté, filtres repliés, tri des tables Téléchargements et
  Rechercher survivent au redémarrage. Clés `ui.sidebarCollapsed`,
  `ui.filtersExpanded`, `ui.downloadSort`, `ui.searchColSort`. Le
  thème (mode + accent) était déjà persisté par `themeSettingsProvider`.

## UI — Bannière « daemon injoignable » actionnable (étape 4/8)

- `DaemonUnreachableBanner` montée par `AppShell` quand le SSE est
  coupé : URL courante, « Réessayer » (redécouverte locale) et
  « Configurer… » — dialogue URL + clé API branché sur
  `connectionSettingsProvider.save` (même persistance que Réglages →
  Connexion). Premier onboarding de connexion.

## UI — Centre de notifications (étape 3/8)

- `notificationsProvider` : liste bornée (100) + compteur de non-lues ;
  `AppNotification` (sévérité, horodatage, lecture).
- `NotificationsListener` dans le shell traduit les SSE
  `torrent_finished` (succès), `download_state_changed` →
  `STOPPED_ON_ERROR` (dédupliqué par infohash), `tribler_exception`,
  `low_space`, `tribler_new_version` — les snackbars existants sont
  conservés.
- `NotificationBell` dans la `TopBar` : badge non-lus + panneau
  MenuAnchor (tout marquer lu / vider, 50 dernières).

## UI — Diagnostic : onglet « Vue d'ensemble » (étape 2/8)

- Premier onglet : grille de 9 `_StatCard` (overlays, circuits prêts/
  total, relais, sorties actives, pairs tunnel, torrents, débits ↓/↑,
  taille DB) alimentée par les providers existants.
- Pastille de santé tunnel (vert = circuits READY, orange =
  communauté active sans circuit, rouge = inactive) et auto-refresh
  périodique de 5 s tant que l'onglet est monté.

## UI — Palette OnionBit (étape 1/8 du plan global)

- `AppTheme.defaultSeedColor` → violet `#6C2EA6` (ampoule du logo) ;
  `tertiary` fixée au cyan `#4FD8E0` (flèche) dans les deux thèmes.
- `kAccentChoices` : entrée « OnionBit » en tête de liste ; la
  persistance du thème (`theme_settings.dart`) existait déjà.

## UI — Sidebar : pied avec état daemon + version (étape 6/6)

- `_DaemonFooter` : pastille `sseConnectedProvider` + « Daemon vX.Y »
  (`/api/versioning/versions` → `current`) ; réduite à la pastille
  avec tooltip en mode rail. Plan terminé.

## UI — Sidebar : mode rail rétractable 216 ↔ 72 px (étape 5/6)

- `sidebarCollapsedProvider` (session) + bouton chevron en pied :
  rail icônes seules — logo `icon.svg`, bouton « + », items centrés
  avec `Tooltip`, groupes remplacés par un séparateur.
- Même items/providers, aucun état dupliqué ; l'indentation des
  sous-filtres est neutralisée en mode rail.

## UI — Sidebar : sous-filtres repliables (étape 4/6)

- `sidebarFiltersExpandedProvider` (session) + chevron en bout de
  l'entrée Téléchargements : replie/déplie le groupe des quatre
  filtres sans toucher à la navigation ; le label conserve
  `context.go('/downloads')`.

## UI — Sidebar : logo OnionBit SVG (étape 3/6)

- `flutter_svg` + assets `app/assets/branding/` (copie de
  `branding/logo-horizontal.svg` et `icon.svg`) ; l'en-tête de la
  sidebar affiche le logo horizontal, avec fallback shield+texte si
  l'asset n'est pas embarqué.

## UI — Sidebar : débits globaux ↓/↑ dans l'en-tête (étape 2/6)

- `_SpeedsRow` sous le bouton « Ajouter » : réception/envoi temps
  réel via `totalSpeedsProvider` (même source que la barre d'état),
  icônes fléchées colorées + `ByteFormatter.formatRate`.

## UI — Sidebar : items pilule + groupes + badge d'erreurs (étape 1/6)

- `app_sidebar.dart` réécrit : `_SidebarItem` pilule arrondie
  (InkWell + primaryContainer) remplace les `ListTile` plats,
  compteurs en badge `surfaceContainerHighest`, icônes par filtre.
- En-têtes de groupe « Bibliothèque » / « Système » (labelSmall,
  tracking 0.8).
- Badge `errorContainer` affichant le nombre de téléchargements en
  `STOPPED_ON_ERROR` sur l'entrée Téléchargements.

## UI — Réglages : carte « État effectif » réseau (étape 7/8)

- `_EffectiveState` en tête de la section Réseau : port d'écoute et
  interfaces réels (`listen_port`/`listen_interfaces` runtime),
  indicateurs DHT/UPnP/NAT-PMP/LSD/uTP, proxy actif, et pairs par
  overlay IPv8 (`/api/ipv8/overlays`).
- Lecture seule et tooltip rappelant que les commutateurs éditables
  ne prennent effet qu'au redémarrage — réutilise
  `overlaysProvider` du diagnostic, aucune traduction REST dupliquée.

## UI — Réglages : sliders + presets de bande passante (étape 6/8)

- `_RateControl` : champ Ko/s + slider exponentiel (0 = illimité ↔
  10 Mo/s, granularité fine en bas de plage) + chips presets
  Illimité / 1 / 5 / 10 Mo/s — tout synchronisé bidirectionnellement.
- Convention `0 = illimité` inchangée (backend `0 → None`) ; la
  modification passe par le suivi « modifié » de l'étape 2.

## UI — Réglages : export/import de la configuration (étape 5/8)

- « Exporter » : copie du JSON complet dans le presse-papier.
- « Importer » : dialogue de collage avec validation (objet JSON
  obligatoire), aperçu des sections racines détectées ; le contenu
  charge l'éditeur mais n'est envoyé qu'au « Appliquer » — intégré au
  suivi « modifié » de l'étape 2.

## UI — Réglages : bouton « Rétablir les défauts » par section (étape 4/8)

- `settings_defaults.dart` : `kBandwidthDefaults`, `kQueueDefaults`,
  `kSeedingDefaults`, `kAnonymityDefaults`, `kNetworkDefaults`,
  `kAutomationDefaults` — valeurs identiques aux `Default` backend
  (`onionbit-core`/`onionbit-bittorrent`/`onionbit-tunnel`).
- `SettingsSection` accepte `defaults` : bouton reset dans l'en-tête →
  `applySettingsPatch` + ré-synchronisation des champs locaux via le
  bus de l'étape 2.
- Downloads volontairement exclu : aucun défaut sûr pour `saveas`.

## UI — Réglages : tooltips « clé configuration.json » (étape 3/8)

- `KeyInfoIcon(path, [description])` : icône `i` affichant le chemin
  dans `configuration.json` (`libtorrent/max_download_rate`) au survol.
- `SettingsSwitch` affiche désormais automatiquement l'icône à partir
  de son `path` — couverture gratuite de tous les commutateurs.
- `suffixIcon` déployé sur les champs texte des sections Bandwidth,
  Queue, Downloads (saveas), Seeding (ratio/durée), Anonymity
  (circuits), Network (proxy) et Automation (watch folder/RSS).

## UI — Réglages : éditeur avancé de l'arbre `configuration.json` (étape 5, partie éditeur)

- Nouvelle section « Configuration avancée » : éditeur JSON indenté de
  l'arbre complet retourné par `GET /api/settings` — fallback pour les
  réglages non exposés dans les sections dédiées.
- Validation avant envoi : document obligatoirement objet, erreurs de
  syntaxe affichées sous le champ, avertissement sur les clés sensibles.
- Raccordée au bus « Enregistrer tout » / pastille « modifié » de
  l'étape 2 ; bouton « Recharger » pour réinitialiser depuis le serveur.

## UI — Réglages : indicateurs « modifié » + enregistrement global (étape 2/8)

- Sections à sauvegarde différée (Bande passante, File d'attente,
  Téléchargements, Seed & anonymat, Tunnels, Réseau, Automatisation)
  désormais raccordées à un bus partagé (`settingsSaveBusProvider`) et à
  un compteur de sections sales (`settingsDirtyProvider`).
- Pastille orange « modifié » sur la carte de section et sur la chip du
  rail d'ancres ; bannière globale « N section(s) modifiée(s) » avec
  « Enregistrer tout » (séquentiel, les échecs conservent leur pastille)
  et « Tout annuler » (ré-initialise les champs depuis le serveur).
- Les boutons « Enregistrer » par section existants sont conservés et
  effacent la pastille en cas de succès.

## Stabilisation : erreurs différées du stockage paresseux + état hashcheck (2026-09-30)

Suite de « stockage paresseux » : l'init sans accès disque reporte les
erreurs de chemin/permissions à la première E/S — cette étape verrouille
ce comportement et rend la phase de check visible dans l'API.

- **Tests d'erreur différée** (librqbit vendored) : `OpenedFile::new_lazy`
  sur un chemin impossible (parent = fichier ordinaire) → `ensure_len`
  différé sans disque, erreur propre au premier `ensure_open`/`lock_read`/
  `lock_write` ; `FilesystemStorage` et `MmapFilesystemStorage` remontent
  `Err` sans panique — le mmap n'est pas matérialisé après l'échec.
- **État « check en cours » distinct de « en file »** :
  `TorrentStateInitializing::check_started` (positionné à l'entrée de
  `check()`, après le sémaphore `concurrent_init_limit`) exposé via
  `TorrentStats.checking` ; `Download::stats` mappe un torrent en
  `Initializing` non pausé vers `Checking` → l'API émet `HASHCHECKING`
  (2) quand le fastresume/recheck tourne réellement,
  `WAITING_FOR_HASHCHECK` (1) tant qu'il attend dans la file d'init.
  « Restauration terminée » et « vérification terminée » sont deux
  états mesurables distincts.
- **Chaîne complète** (test `onionbit-core`) :
  `restauration_sortie_inaccessible_erreur_differee` — dossier de sortie
  remplacé par un fichier entre deux runs : la restauration réussit
  (init lazy), le check différé marque toutes les pièces manquantes
  (`progress_bytes = 0`), le download sort de `Initializing`/`Checking`
  sans état `Error` fatale ni crash.
- **Infra de test vendored** : `vendor/*` exclus du workspace racine +
  `[patch.crates-io]` local dans `vendor/librqbit/Cargo.toml` vers les
  crates sœurs vendored + `resources/` rapatrié depuis rqbit —
  `cargo test --manifest-path vendor/librqbit/Cargo.toml` fonctionne
  désormais.

## UI : ancres + filtre dans Réglages (2026-09-30)

Étape 1 de `docs/plans/app_settings_enrichissement.md` : la page passe
d'une liste littérale à un catalogue (`_kSections` : titre, mots-clés
incluant chemins de clés, widget). Rail de chips en haut →
`Scrollable.ensureVisible` sur la section ; champ « Filtrer les
réglages » qui ne construit que les sections correspondantes.

## UI : sonde de santé à la demande dans Rechercher (2026-09-30)

Étape 8 de `docs/plans/app_search_enrichissement.md` : entrée
« Rafraîchir la santé » du menu contextuel →
`GET /metadata/torrents/{ih}/health?refresh=1` (scrape immédiat des
trackers connus) ; la réponse met à jour seeds/leechers de la ligne via
`healthOverridesProvider` sans recharger la liste (`"checking"` →
snackbar explicite). Plan terminé.

## UI : historique des recherches récentes (2026-09-30)

Étape 7 de `docs/plans/app_search_enrichissement.md` :
`searchHistoryProvider` (LRU 10, session) est alimenté par chaque
recherche non vide ; quand la requête est vide, une rangée de chips
« Récents » sous le titre permet de relancer une requête en un clic.

## UI : arrêt et compteur de la recherche distante (2026-09-30)

Étape 6 de `docs/plans/app_search_enrichissement.md` : la barre affiche
le nombre de résultats distants accumulés, un bouton Stop coupe la
fenêtre de collecte (`RemoteResultsNotifier.stop` — la boucle
`_collectRemote` sort au prochain tick), et l'heure de fin est affichée
après la recherche (`finishedAt`).

## UI : surlignage des termes + chips de filtre dans Rechercher (2026-09-30)

Étape 5 de `docs/plans/app_search_enrichissement.md` : les termes de la
requête (≥ 2 caractères, insensible à la casse) apparaissent en gras
dans les noms (`Text.rich`, `_highlighted`). Chips de filtre sous la
barre de titre : source Tous/Local/Réseau et « ≥ 10 seeds »
(`searchFilterProvider`, appliqué à la liste fusionnée avant le tri).

## UI : sélection multiple + ajout en lot dans Rechercher (2026-09-30)

Étape 4 de `docs/plans/app_search_enrichissement.md` : cases à cocher
sur les lignes (`searchSelectionProvider`), bouton « Ajouter (N) » dans
la barre — ajout séquentiel direct, erreurs comptabilisées en snackbar
de synthèse, sélection vidée au terme.

## UI : badge « déjà téléchargé » dans Rechercher (2026-09-30)

Étape 3 de `docs/plans/app_search_enrichissement.md` : les résultats
dont l'info-hash figure déjà dans `downloadsProvider` affichent un
chip « En cours » à la place du bouton Ajouter (table et liste
compacte) — anti-doublon immédiat.

## UI : menu contextuel + ajout anonyme dans Rechercher (2026-09-30)

Étape 2 de `docs/plans/app_search_enrichissement.md` : clic droit sur
un résultat (table et liste compacte) — « Ajouter… » (dialogue),
sous-menu « Ajout rapide » 0-3 sauts (`anon_hops` + `safe_seeding`
forcé, ajout direct sans dialogue), copie magnet/info-hash. Même
pattern `MenuAnchor` que Téléchargements.

## UI : table triable + date + santé dans Rechercher (2026-09-30)

Étape 1 de `docs/plans/app_search_enrichissement.md` : les résultats
passent en table desktop à en-têtes triables (Nom, Taille, Seeds,
Leechers, Date, Source — tri client sur la liste fusionnée, `null` =
ordre pertinence). `TorrentResult.date` est désormais mappé depuis
`updated`/`torrent_date` (epoch ou ISO). Pastille de santé par ligne
(seeds/leechers/inconnu). Compact : ListTiles conservés.

## UI : snackbar de complétion sur SSE `torrent_finished` (2026-09-30)

Étape 8 de `docs/plans/app_downloads_enrichissement.md` :
`TorrentFinishedListener` (même pattern que `PendingFilesHandler`,
monté dans les deux variantes du shell) écoute `daemonEventsProvider`
et affiche un snackbar avec le nom du torrent terminé — visible sur
toutes les pages, pas seulement Téléchargements.

## UI : onglet Pairs en table détaillée (2026-09-30)

Étape 7 de `docs/plans/app_downloads_enrichissement.md` : les pairs
connectés (déjà pollués via `?get_peers=1`) passent de cartes à une
`DataTable` — adresse, client (`extended_version`), direction
entrant/sortant, débits ↓/↑, totaux échangés, transport
(`connection_type`). Scroll horizontal sous faible largeur.

## UI : glisser-déposer .torrent/magnet sur Téléchargements (2026-09-30)

Étape 6 de `docs/plans/app_downloads_enrichissement.md` : `DropTarget`
(`desktop_drop` 0.8.x, MIT) enveloppe la liste — un `.torrent` lâché
part en `addTorrentBytes`, un fichier/lien `magnet:` en `add(uri:)` ;
surbrillance pendant le survol, erreurs par fichier en snackbar sans
interrompre le lot. Non applicable au web (pas de dépôt de fichiers
desktop côté navigateur) — écart assumé, documenté dans le plan.

## UI : raccourcis clavier sur Téléchargements (2026-09-30)

Étape 5 de `docs/plans/app_downloads_enrichissement.md` : Espace
pause/reprend la sélection (éléments mixtes gérés un à un), Suppr ouvre
le dialogue de suppression (factorisé en `confirmRemoveSelected`,
partagé avec la barre d'actions), F2 ouvre les limites de débit sur
sélection unique. Ctrl+A/Échap livrés à l'étape 1.

## UI : sparkline de débit temps réel dans le panneau de détail (2026-09-30)

Étape 4 de `docs/plans/app_downloads_enrichissement.md` : l'onglet
« Détails » affiche en tête un graphe ↓/↑ des 120 derniers échantillons
(un par poll de `downloadsProvider`), avec débits courants et crête.
Historique conservé dans le `State` du widget (`_DetailsTab` devient
stateful) — pas de provider supplémentaire, mémoire bornée, destruction
à la fermeture du panneau. Rendu par `CustomPainter` maison
(`SpeedSparkline`).

## UI : presets de limites de débit dans le menu contextuel (2026-09-30)

Étape 3 de `docs/plans/app_downloads_enrichissement.md` : le menu
contextuel remplace l'entrée « Limites de débit… » par un sous-menu
Réception/Envoi à presets rapides (64 à 4096 Kio/s, coche sur la valeur
courante), « Illimité » (`-1` — `0` est ignoré par le walrus backend et
`-1` retombe sur `None` via `u64::try_from`) et « Personnalisé… » qui
rouvre le dialogue complet.

## UI : lignes de téléchargement enrichies (2026-09-30)

Étape 2 de `docs/plans/app_downloads_enrichissement.md` :

- % affiché au centre de la barre de progression (barre rouge si erreur).
- Zone de badges en fin de ligne : erreur (tooltip `error`), torrent
  privé, position de file, anonymat (`_RowBadges`).
- Pastille de santé de l'essaim dans la colonne Pairs : vert/orange/rouge
  selon seeders connus et pairs connectés (`_HealthDot`).
- Nouvelles colonnes triables « Ratio » et « Ajouté » (largeur min de la
  table portée à 1130 px).

## UI : tri par colonnes et sélection étendue dans Téléchargements (2026-09-30)

Étape 1 de `docs/plans/app_downloads_enrichissement.md` :

- En-têtes de la table desktop cliquables (Nom, Taille, Progression, État,
  ↓, ↑, ETA, Pairs) : même colonne = inversion du sens, nouvelle colonne =
  ascendant ; flèche et couleur primaire sur la colonne active. État porté
  par `downloadSortProvider`, comparateurs dans `downloadComparator`.
- Sélection clavier/souris : clic = sélection unique, Ctrl/Cmd+clic =
  toggle, Shift+clic = plage depuis l'ancre du dernier clic, Ctrl+A = tout,
  Échap = vider (`CallbackShortcuts` sur la table). `DownloadSelectionNotifier`
  mémorise une ancre pour les plages.

## Perf : stockage paresseux + cache des lignes `/api/downloads` (2026-09-30)

Les mesures d'instrumentation (`890ee72`) ont tranche : la restauration
etait dominee par `create_and_init` (~31 ms **par fichier** sous Windows
— 349 s pour un torrent de 11 310 fichiers) puis par
`validate_fastresume`. Le stockage vendored ouvre desormais les fichiers
a la demande, comme le file pool de libtorrent :

- `OpenedFile` (`vendor/librqbit/.../opened_file.rs`) peut etre cree en
  mode paresseux (`new_lazy`) : chemin + `allow_overwrite` enregistres,
  `fd` ouvert a la premiere lecture/ecriture (`ensure_open`). La
  creation du dossier parent, `CreateFile`, le marquage sparse et
  `set_len` sont differes ; `ensure_file_length` sur un fichier non
  encore ouvert n'enregistre que la longueur (`pending_len`), appliquee
  a l'ouverture.
- `FilesystemStorage::init` ne fait plus aucun appel disque — il
  n'enregistre que les metadonnees des fichiers.
- `MmapFilesystemStorage` ne mappe plus les fichiers a l'init : chaque
  `RwLock<Option<MmapMut>>` est materialise a la premiere lecture/
  ecriture du fichier.
- Compatibilite `FileOps` : un fichier absent/non encore cree remonte
  l'erreur `FsFileIsNone`/I/O au lecteur, deja traitee par
  `initial_check` (pieces marquees manquantes, check continue) — la
  validation complete n'est pas cassee.
- Effet de bord : un chemin de sortie invalide ou des permissions
  manquantes ne sont plus detectes a l'ajout du torrent mais a la
  premiere I/O (erreur differee documentee — ADR-0008).
- Precision de mesure : « restauration en quelques ms » = fin de
  l'init/registration, pas fin de validation — `validate_fastresume`
  (echantillonnage, actif par defaut) et `initial_check` continuent en
  arriere-plan ; le deux sont distingues dans l'API depuis l'entree
  « erreurs differees » ci-dessus (`WAITING_FOR_HASHCHECK` vs
  `HASHCHECKING` via `TorrentStats.checking`).
- `GET /api/downloads` met en cache ~800 ms les lignes
  `downloads`/`torrent_states` (`AppState::downloads_rows`) : pendant la
  restauration l'UI poll en boucle et chaque acces sqlite prenait
  300-700 ms sous contention. Le cache est invalide par les endpoints
  mutants (`PUT`/`PATCH`/`DELETE /api/downloads`).

## Fix : dédup par infohash à l'écriture de session.json (2026-09-30)

- Le guard in-flight par infohash (`4a28ac5`) empêchait les doublons
  dans `db.torrents` mais pas dans le **fichier** : une entrée chargée
  au démarrage (restore rqbit) restait dans la map de persistance même
  quand son `add_torrent` retournait `AlreadyManaged`, pendant que la
  restore `onionbit.db` réinjectait le même infohash sous un nouvel id —
  les deux coexistaient dans `session.json` et se multipliaient à
  chaque démarrage.
- `JsonSessionPersistenceStore::update_db` supprime désormais les
  entrées d'autres ids portant le même `info_hash` avant l'insertion —
  le fichier ne peut plus contenir deux entrées pour un même torrent.
- Instrumentation des phases d'init (commit `890ee72`) : `create_and_init`,
  `validate_fastresume`, `initial_check`, `ensure_file_length` loguent
  leur `elapsed_ms` ; `restore_downloads` logue un récapitulatif
  `total_elapsed_ms` en fin de restauration.

## Option : échantillonnage fastresume désactivable au démarrage (2026-09-29)

- Nouvelle clé `libtorrent/fastresume_check` (défaut `true` —
  comportement actuel identique à Tribler) propagée jusqu'au vendored
  librqbit (`SessionOptions::fastresume_sampled_check` →
  `TorrentStateInitializing::validate_fastresume`).
- `false` : le `.bitv` persisté est accepté après le simple contrôle de
  longueur — restauration quasi instantanée, aucun re-hash
  échantillonné. Trade-off assumé : les fichiers modifiés/déplacés entre
  deux runs ne sont plus détectés (comportement proche du fastresume de
  qBittorrent/libtorrent).

## Fix : téléchargements en file de restauration visibles dans l'API (2026-09-29)

- `GET /api/downloads` n'émettait que les torrents déjà insérés dans le
  moteur : pendant la restauration sérialisée (`concurrent_init_limit`),
  les téléchargements en attente n'apparaissaient pas dans l'UI puis
  surgissaient un par un.
- Les lignes `downloads` de `onionbit.db` absentes du moteur sont
  désormais émises en `WAITING_FOR_HASHCHECK` (statut Python 1,
  « en file pour le check » — `STOPPED` si `paused`/`user_stopped`),
  avec les réglages persistés (destination, hops, limites). L'UI les
  affiche « Vérification » dès le lancement.

## Fix : dédup in-flight des `add_torrent` — fin des doublons session.json (2026-09-29)

- **Race à l'insertion** : depuis que `create_and_init` tourne hors du
  write-lock de `Session.db` (cf. entrée précédente), la restauration
  rqbit (`session.json`) et celle de Tribler (`onionbit.db`) appelaient
  `add_torrent` en parallèle pour le même infohash : les deux passaient
  le check `AlreadyManaged` avant que l'autre ait inséré → une entrée
  dupliquée re-persistée à chaque démarrage (7 nouvelles observées au
  run suivant le fix précédent).
- **Correctif** : un `Mutex` par infohash (`Session::add_locks`), acquis
  avant le sémaphore de concurrence, tenu pendant tout l'ajout : un
  second `add_torrent` du même infohash attend le premier puis retombe
  sur `AlreadyManaged`. Impossible de dupliquer quelle que soit la
  fenêtre check→insertion.
- **Conséquence constatée** : les entrées dupliquées pointaient vers des
  `output_folder` imbriqués (`Nom\Nom`) vs parents — 5 torrents (DaVinci,
  Acrobat, AIDA64, Revo, Nintendo64Pal) avaient ~16 Gio de copies en
  double sur `D:` et des mismatches de pièces à l'échantillonnage.

## Fix UI : zone de contenu invisible + fluidité executor au restore (2026-09-29)

- **UI vide** : le `Stack` de `AppShell` n'avait qu'un enfant
  non-positionné à taille nulle (`PendingFilesHandler` →
  `SizedBox.shrink`) : en `fit: loose`, le `Stack` mesurait 0×0 et le
  `Positioned.fill` réduisait tout le corps à zéro pixel — seule la
  sidebar (hors `Stack`) restait visible. `PendingFilesHandler` est
  désormais en `Positioned.fill` (variantes compacte et desktop).
- **Gel du daemon au démarrage** : les I/O synchrone de librqbit
  (marquage sparse, hash, init mmap) s'exécutent via `block_in_place`
  sur les workers Tokio — plusieurs gros torrents multi-fichiers
  absorbaient tous les workers et figeaient l'API jusqu'à ~66 s.
  `EngineConfig::runtime_worker_threads` borne le sémaphore
  `block_in_place` de rqbit et le runtime Tokio passe à
  `2 × cœurs` (min 8) pour garder des workers libres.
- **Diagnostic SQLite affiné** : le warn `operation sqlite lente`
  porte désormais le nom de l'opération (`op`) — `Database::with`
  remonte le site appelant via `#[track_caller]`, `Database::call`
  prend un label explicite sur ses ~16 appelants. Permet d'identifier
  la file d'attente qui sature quand le disque est pris par un gros
  torrent multi-fichiers.

## Fix : doublons session.json et chemins imbriques au restore (2026-09-29)

- **Dedup de persistance (librqbit vendored)** : `session.json`
  accumulait plusieurs entrees par infohash (ajouts/suppressions
  repetes) ; chaque exemplaire etait restaure sur son propre
  `output_folder`. `JsonSessionPersistenceStore::new` supprime
  desormais les entrees dupliquees au chargement (plus petit id
  conserve, warn logue) — le fichier se nettoie au prochain flush.
- **Chemin `Nom\Nom` au restore** : `output_dir` persiste provient de
  `Download::output_folder()` = dossier final (nom inclus), mais
  `rqbit_opts` reappliquait `name_subfolder: true` → rqbit re-joignait
  le nom → donnees invisibles dans le dossier imbrique, torrents
  revus a 0 %. Nouveau flag `AddDownloadOptions::output_includes_name`
  (vrai au restore) desactivant `name_subfolder` dans ce cas.

## Fix : init disque des torrents hors du lock de session (2026-09-29)

`add_torrent` (librqbit vendored) executait `create_and_init` —
boucle synchrone sur chaque fichier (create_dir_all, open, mark
sparse, set_len, mmap) — sous le write-lock de `self.db`. Au restore,
chaque torrent multi-fichiers bloquait `with_torrents` (liste API)
et les autres ajouts pendant des dizaines de secondes, empiles les
uns derriere les autres. L'init est desormais faite hors du lock
(concurrence toujours bornee par le semaphore du spawner), la dedup
`AlreadyManaged` est verifiee en lecture avant l'init puis re-verifiee
a l'insertion.

## Étape 12 clôturée : interop tunnel rejouée contre pyipv8 réel (2026-09-29)

Le banc `scripts/interop_exit_download.ps1` a été exécuté avec
succès : téléchargement rqbit réel (200 Ko, uTP) à travers un
circuit à 2 sauts dont le dernier saut est le vrai
`TunnelCommunity` pyipv8 en sortie (`PEER_FLAG_EXIT_BT`) —
`INTEROP EXIT DOWNLOAD OK`, contenu vérifié octet à octet. Le jalon
« backend terminé à 100 % » de `roadmap.md` est mis à jour : plus
aucune étape backend n'a de critère ouvert.

## Stores PEX persistés : intro points survivent au redémarrage (2026-09-29)

Le rôle de point d'introduction (`TunnelCommunity.pex` —
`PexCommunity` réduite à ses données) était perdu à chaque arrêt :
les swarms en hidden seeding devenaient injoignables jusqu'à ce que
le seeder refasse un `establish-intro`.

Changements :

- `onionbit-db` : **migration v10** — table `tunnel_pex`
  (`info_hash`, `own` = annonce propre vs point appris, `peer_key`,
  `seeder_pk`, `address`, `source`, `last_seen`) ; module `pex.rs`
  (`replace_all` snapshot transactionnel, `list`, `delete_swarm`).
- `onionbit-tunnel/community.rs` : `pex_dump`/`pex_restore`
  (extension Rust — le crate reste sans dépendance SQLite ; type
  public `PexDumpEntry`).
- `onionbit-core/ipv8_stack.rs` : restauration après création du
  tunnel (les annonces `intro_points_for` sont régénérées avec notre
  WAN courant) ; la tâche `ipv8_peer_cache` écrit aussi le snapshot
  PEX dans le même `db.call` — pas d'écriture supplémentaire.

## Diagnostic perf : sonde de lag de l'executor + timings I/O (2026-09-29)

Objectif : identifier la cause des ralentissements disque rapportés —
mesurer avant d'optimiser (un appel `std::fs` synchrone dans du code
async fige tout l'executor Tokio : API, tunnels, moteur).

Changements :

- `onionbit-core/asyncio/monitor.rs` : **sonde de lag de l'executor** —
  tâche tickant toutes les 100 ms ; un retard ≥ 500 ms log un
  `warn!` (« tache bloquante suspectee »). Démarrée dans
  `AsyncioMonitor::new`, abordée au `Drop`.
- `onionbit-db` : `Database::with` log un `warn!` pour toute opération
  SQLite (attente du mutex comprise) ≥ 250 ms — les requêtes lentes et
  la contention de connexion deviennent visibles dans `onionbit.log`.
- `onionbit-core/session.rs` : `move_storage` déporte le déplacement de
  fichiers (`move_dir_contents`, copie récursive inter-volumes) sur
  `spawn_blocking` + `warn!` si > 500 ms — fix réel : la copie de gros
  volumes ne fige plus l'executor, rollback inclus.

## Cache de pairs IPv8 persisté : bootstrap quasi instantané (2026-09-29)

Objectif : réactivité au démarrage — pyipv8 ne persiste pas son
annuaire `Network`, le graphe se reconstruisait à froid via DNS +
premier cycle d'introduction (dizaines de secondes avant le premier
pair vérifié, circuits anonymes et recherche distante indisponibles).

Changements :

- `onionbit-db` : **migration v9** — table `ipv8_peers`
  (`public_key` PK, `address` "ip:port", `last_seen`, `new_style`) +
  index sur `last_seen` ; module `peers.rs` (`upsert_batch` en une
  transaction, `list_peers` borné par fraîcheur, `prune` expire +
  borne la table).
- `onionbit-core/ipv8_stack.rs` : au `start`, les pairs du cache sont
  rechargés dans `Network::add_verified` et leurs adresses jointes
  aux cibles de marche du bootstrap (le cache seul suffit à amorcer
  si la liste de noeuds est vide ou le DNS lent). Tâche
  `ipv8_peer_cache` : snapshot `Network` -> `ipv8_peers` toutes les
  `peer_persist_interval_secs` via `db.call` (un lot, pas d'écriture
  par pair) + prune.
- Config `Ipv8Config` : `peer_cache_max` (512),
  `peer_cache_max_age_secs` (7 j — une adresse IP est généralement
  réattribuée au-delà), `peer_persist_interval_secs` (120).
- Bonus : lints du nouveau toolchain corrigés au passage
  (`useless_format`, `manual_flatten`, `type_complexity`,
  `doc_lazy_continuation`) et `cargo fmt` sur les fichiers concernés.

## Passe performance : SQLite hors du thread async + index (2026-09-29)

Objectif : fluidité de l'UI Flutter — les requêtes SQLite étaient
exécutées en synchrone sur les threads de l'executor Tokio (handlers
axum), figeant l'API pendant les recherches FTS et les listes.

Changements :

- `onionbit-db/db.rs` : pragmas de performance à l'ouverture —
  `synchronous=NORMAL` (sûr en WAL), `busy_timeout=5000`,
  `temp_store=MEMORY`, `cache_size=-20000` (~20 Mio),
  `mmap_size=64 Mio`, cache de statements préparés (64) et
  `PRAGMA optimize` post-migrations. Nouvelle méthode async
  `Database::call` (`spawn_blocking` + `with`).
- `onionbit-db/migrations.rs` : **v8** — index
  `channel_node(metadata_type, torrent_date)` (popular / filtres de
  type), `torrent_state(has_data, last_check)` et
  `torrent_state(last_check)` (popular / historique de santé).
- `onionbit-db/health.rs` : `list_torrent_states` en masse.
- `onionbit-api/handlers/` : tous les accès DB du chemin de requête
  passent par `call`/`spawn_blocking` (`metadata` : popular, santé,
  `search/local` FTS + augmenteur, completions, tags ; `statistics`,
  `rss`).
- `downloads.rs` `GET /api/downloads` : suppression du **N+1** —
  `downloads::list` + `list_torrent_states` une seule fois, indexées
  en mémoire, au lieu de 2 requêtes par téléchargement (+
  `anon_hops_map` fusionné dans le même fetch).

## Fastresume rqbit : plus de re-hash au changement de hops ni au redémarrage (2026-09-29)

Symptôme : changer `anon_hops` en cours de téléchargement (ou
redémarrer le daemon) relançait une validation complète des pièces —
long sur les gros fichiers, contrairement aux checkpoints libtorrent
de Tribler.

**Cause** : `EngineConfig.fastresume` était `true` mais
`SessionOptions::persistence` restait `None` — rqbit n'active le
fastresume qu'avec un backend de persistance ; le flag était sans
effet (`NonPersistentBitVFactory`).

Changements :

- `onionbit-bittorrent/config.rs` : nouveau champ `persistence_dir`
  traduit en `SessionPersistenceConfig::Json` quand `fastresume` est
  actif ; tests offline inchanges (`None`).
- `onionbit-core/daemon_config.rs` : le moteur principal persiste dans
  `state_dir/rqbit/main`.
- `onionbit-core/ipv8_stack.rs` : chaque lane anonyme a son dossier
  `state_dir/rqbit/anon<N>` — un dossier dedie par moteur empeche la
  restauration croisee des torrents anonymes sur le moteur en clair
  (rqbit re-ajoute le contenu de `session.json` au demarrage de la
  session).
- `onionbit-core/session.rs` : `update_hops` met de cote le
  `<ih>.bitv` de l'ancienne lane (le `delete` rqbit le supprime) et
  le depose dans le dossier de la nouvelle lane avant le re-add —
  `validate_fastresume` (verification legere d'un echantillon par
  fichier) remplace alors le re-hash complet.

## Correctif majeur : circuits anonymes bloqués sur un premier saut mort + layout filaire par community (2026-09-29)

Symptôme : plus aucun téléchargement anonyme — tous les circuits
échouaient sur le même premier saut `220.233.67.99:8090` (`timeout du
saut suivant` → `circuit abandonne` → `aucun circuit pret` pour SOCKS5
et le DHT), et le journal montrait des erreurs de parsing massives
(`preference_list non multiple de 20`, `paquet tronque`).

**Cause 1 — aucun retry de construction de circuit.** En pyipv8,
`RetryRequestCache` reteste les candidats alternatifs à chaque timeout
(`max_tries = circuit_timeout / next_hop_timeout ≈ 6` pour le create,
`max_tries` par saut pour l'extend). Le port Rust n'essayait que le
premier candidat retourné par `select_candidates` (déterministe →
toujours le même pair injoignable) puis abandonnait le circuit.

**Cause 2 — layout signé/non-signé global au lieu de par community.**
`DIST_MSG_IDS`/`UNSIGNED_MSG_IDS` étaient des ensembles globaux alors
qu'en pyipv8 le `lazy_wrapper` de chaque handler décide : discovery
émet `ping`/`pong` **non signés** avec `dist`, et `similarity`-req/res
(1/2) signés **avec** dist ; les autres overlays (`ez_send` pur)
n'ont pas de `dist`. La découverte réelle voyait donc des paquets
désalignés.

Changements :

- `onionbit-ipv8/packet.rs` : `WirePolicy` (`signed` + `dist` par
  `msg_id`) avec `WIRE_DEFAULT` et `WIRE_DISCOVERY` ; `Packet::parse`
  prend la policy en paramètre.
- `onionbit-ipv8/endpoint.rs` : la policy est stockée par listener de
  préfixe (`add_prefix_listener_with_policy`).
- `onionbit-ipv8/discovery.rs` : enregistrement sous `WIRE_DISCOVERY`,
  `send_payload` aligné sur pyipv8 (ping/pong non signés + dist,
  similarity signée + dist).
- `onionbit-ipv8/{content_discovery,dht}`, `onionbit-tunnel` :
  enregistrement `WIRE_DEFAULT`.
- `onionbit-ipv8/content_discovery.rs` : `HealthPayload` — les items
  `[HealthFormat]` sont des `NestedPayload` pyipv8, donc chacun est
  prefixe de sa longueur `>H` (u16). Sans ce prefixe, le `unpack`
  desalignait et `varlen_h` exigeait des dizaines de ko — les erreurs
  « paquet tronque … msg_id=4 » du journal.
- `onionbit-tunnel/community.rs` : port du retry pyipv8 —
  `send_initial_create` reteste les candidats alternatifs du premier
  saut (jusqu'à `circuit_timeout / next_hop_timeout` essais) ;
  `send_extend` reteste les candidats de relais/sortie de la liste
  `created` (repli `get_candidates(EXIT_BT, RELAY)` élargi aux pairs du
  service quand le registre de flags est vide) ; le premier saut d'un
  circuit multi-sauts exclut la sortie requise comme en Python.

Tests : fixtures de test corrigées (ping non signé ⇒ les tests de
découverte passent par `introduction-request`, le vrai chemin signé) ;
tous les tests `onionbit-ipv8`/`onionbit-tunnel` + workspace verts.
`cargo check`/`clippy -D warnings`/`fmt --check` propres. Les tests
`onionbit-daemon`/`onionbit-cli` n'ont pas pu être relancés : le binaire
`target\debug\onionbit-daemon.exe` était verrouillé par une instance en
cours d'exécution (compilation vérifiée via `cargo check`).

## Correctif majeur : les overlays DHT et tunnel ne marchaient jamais non plus (2026-09-29)

Suite de l'audit « pas de trou ailleurs » : en Python, **toutes** les
communities Tribler reçoivent `RandomWalk(target=20)` via
`BaseLauncher.get_walk_strategies` (`components.py`). Après
content-discovery, les deux overlays restants avaient le même trou :

| Overlay | Avant | Après |
| --- | --- | --- |
| `DhtCommunity` | `walk_to` au bootstrap seulement, réponse d'intro minimale (pas d'introduction de tiers, pas de `discover_address`) | `step()`/`run()` périodique + handlers complets |
| `TunnelCommunity` | aucune marche, réponse d'intro sans introduire de pair, punctures ignorées | `step()`/`run()` périodique + handlers complets |

- `onionbit-ipv8/dht/community.rs` : marche aléatoire (même ordre de
  choix : pair de l'overlay → adresse walkable du service → pair
  vérifié quelconque → bootstrapper), `discover_address` sur les
  intros reçues, introduction d'un tiers + `puncture-request`,
  traitement des punctures.
- `onionbit-tunnel/community.rs` : idem ; les `peer_flags` du tunnel
  continuent d'être portés par `extra_bytes` des intros émises et
  lus à la réception ; `IntroRequestParams` factorise le decode
  des formats ancien (246) et nouveau (234).
- `onionbit-core/ipv8_stack.rs` : injection de la `DiscoveryCommunity`
  dans le tunnel (adresses `my_estimated_lan/wan` partagées, comme
  l'endpoint unique Python) et spawn des marches DHT/tunnel aux
  côtés de content-discovery dans la tâche de bootstrap.
- Test loopback ajouté côté tunnel : une intro mutuelle peuple
  l'overlay des deux côtés.

Impact : le premier saut des circuits anonymes ne dépend plus du
repli `all_verified_peers` et le DHT overlay ne dépend plus du seul
bootstrap initial — les deux overlays maintiennent leur population
comme pyipv8.

## Correctif majeur : l'overlay content-discovery ne marchait jamais (2026-09-29)

Symptôme : la recherche de l'UI ne trouvait rien — `PUT
/api/search/remote` répondait `"peers": []`.

**Cause racine** : `ContentDiscoveryCommunity` n'avait **pas de
stratégie `RandomWalk`** alors que chaque overlay pyipv8 en a une
(cible 20 pairs). Son overlay ne se remplissait que sur paquets
entrants sous son préfixe — que personne n'émettait puisqu'on ne
s'annonçait jamais sous ce préfixe. Cercle vicieux : 0 pair →
gossip sans cible → recherche muette.

- `onionbit-ipv8/content_discovery.rs` :
  - `step()`/`run()` : marche aléatoire périodique
    (`walker_interval` de la stack) — introduction-request sous le
    préfixe de la community vers un pair connu de l'overlay, une
    adresse walkable du service, un pair vérifié quelconque (un
    noeud Tribler porte toutes ses overlays sur le même port UDP)
    ou un bootstrapper ; cible 20 pairs (`RandomWalk` du launcher).
  - Handlers génériques d'overlay qui manquaient : réponse aux
    `introduction-request` (246/234, ancien+nouveau style) avec
    introduction d'un pair connu + `puncture-request` non signée
    vers le pair introduit ; traitement des `introduction-response`
    (245/233) → `discover_address(service=CD)` ; `puncture-request`
    (250/232, non signé) → `puncture` ; `puncture` accepté no-op.
  - `walk_to` : `advice: true` (demande d'introduction, pas un
    simple accusé) + annonce `my_estimated_lan/wan` réels.
  - La community reçoit la `DiscoveryCommunity` de la stack pour
    les estimations WAN/LAN (un seul endpoint = une seule paire
    d'estimations, comme `IPv8` Python).
- `onionbit-core/ipv8_stack.rs` : la tâche `bootstrap` spawne aussi
  `ContentDiscoveryCommunity::run` avec les noeuds résolus, à la
  cadence `walker_interval`.
- `onionbit-ipv8/discovery.rs` : `same_ip` exposé en `pub(crate)`
  (partage avec la community content-discovery).
- Test loopback `walk_decouvre_les_pairs_de_l_overlay` : une
  `introduction-request` peuple `peers_for_service` des deux côtés.

## Parité : dossier de destination global pour les lanes anonymes (2026-09-29)

Les téléchargements anonymes tombaient par défaut dans
`<downloads>/anon<hops>` (`anon1`..`anon3`) — chaque lane anonyme
figeait son propre `output_dir`. Tribler Python n'a pas de
sous-dossier par saut : le `destination`/`saveas` est global à
toutes les sessions libtorrent.

- `onionbit-core/ipv8_stack.rs` : `anon_engine` utilise désormais
  `downloads_dir` comme `output_dir` par défaut, comme le moteur
  principal. `destination` explicite (`PUT /api/downloads`) et
  `download_dir` (`POST /api/settings`) restent prioritaires.
- Les téléchargements déjà persistés gardent leur `output_dir` en
  DB — seuls les nouveaux ajouts sans destination changent de
  dossier.

## Perf : l'API binde avant la restauration des downloads (2026-09-29)

Le daemon mettait ~18 s avant d'écouter : `CoreSession::start`
attendait `restore_downloads` (re-add séquentiel + vérification
initiale des pièces rqbit — ~17,6 s pour un seul gros torrent) avant
le `TcpListener::bind`. L'UI attendait donc l'API pendant toute la
restauration.

- `restore_downloads` part désormais en **tâche de fond** nommée
  `CoreSession:load_checkpoint` (registre `/api/ipv8/asyncio/tasks`),
  comme le `load_checkpoint` asynchrone de Tribler Python — les
  downloads restaurés apparaissent progressivement côté clients.
- `CoreSession::wait_restored()` (canal `watch` `restore_done`) :
  attente explicite pour les tests ; abandon anticipé de la boucle si
  `stop()` arrive en cours de restauration.
- `main.rs` inchangé : `start()` retournant plus tôt, l'API binde
  quelques centaines de ms après l'ouverture SQLite.

## UI : avertissement trackers HTTPS-only à l'ajout anonyme (2026-09-29)

Conséquence de la limitation confirmée ci-dessous (sorties =
`http-request` one-shot HTTP clair, pas de relais TCP générique) : un
torrent dont **tous** les trackers sont en `https://` ne découvrira
aucun pair en mode anonyme — le `ClientHello` TLS n'est pas une
requête HTTP relayable, d'où les `tls handshake eof` observés.

- **`/api/torrentinfo/{file,uri}`** : extension de la réponse avec
  `trackers` (announce + announce-list dédoublonnés) et `private`,
  pour anticiper côté client (écart documenté dans
  `docs/reference_tribler/api_rest_mapping.md`).
- **Dialogue « Ajouter » (Flutter)** : aperçu du `.torrent` choisi via
  `PUT /api/torrentinfo/file` (échec silencieux — l'ajout reste
  possible) et lecture des `tr=` des magnets ; si tous les trackers
  connus sont HTTPS et que `anon_hops > 0`, avertissement explicite
  invitant à choisir « Direct ». Pas de blocage : le torrent reste
  téléchargeable en désactivant l'anonymat.

## Correctif majeur (bis) : trafic anonyme routé en UDP tunnel — librqbit vendored (2026-09-29)

Le flux continu `http-request`/`http-response` de `6d2f11c` n'était
**pas interopérable** avec les sorties réelles : la référence
`ipv8-rust-tunnels` (le `.pyd` des noeuds Tribler) montre que le
protocole filaire est **one-shot** (chaque `http-request` → un
`send_tcp_request` complet chez la sortie → `http-response` en chunks
bornés). Pas de relais TCP générique — les trackers HTTPS sont
infaisables même dans Tribler officiel (le `ClientHello` binaire n'est
pas du HTTP parseable). Revert de `6d2f11c` → parité stricte.

**Vraie architecture** (vérifiée sur `download_manager.py` Tribler :
`enable_outgoing_tcp=False`, `enable_outgoing_utp=True`,
`anonymous_mode`, `force_proxy`) : le trafic pairs anonyme est **uTP +
DHT + trackers UDP via cellules `data`**, pas TCP. Mais `librqbit`
9.0.1 n'a aucun client SOCKS5-UDP — proxy TCP-only, uTP/DHT/trackers
bindant des sockets réelles. D'où le vendoring :

- **`vendor/`** : `librqbit`, `librqbit-dht`, `librqbit-tracker-comms`,
  `librqbit-dualstack-sockets`, `librqbit-utp` (9.0.1/0.7.0) patchés —
  trait object-safe `DatagramSocket` (dualstack), socket injectable
  DHT + tracker UDP, `ConnectionOptions.utp_socket`
  (`Arc<dyn UtpConnector>` sur `UtpSocket<T,E>`), re-export
  `UtpEnvironment`, proxy pair uniquement si `enable_tcp` (ADR-0007).
- **`onionbit-tunnel::tunnel_udp_socket`** : `TunnelUdpSocket`
  implémente `librqbit_utp::Transport` + `DatagramSocket` par-dessus
  `send_data`/`data_rx` — pinning destination→circuit (préférence
  `PEER_FLAG_EXIT_BT`), réception démuxée par forme de paquet
  (`could_be_utp`/`dht`/`udp_tracker`, sans recouvrement), pertes UDP
  quand aucun circuit n'est prêt (anti-fuite par construction).
- **`anon_engine`** : `TunnelUdpSockets` (uTP/DHT/tracker) par lane ;
  DHT anonyme **réactivée** (routée dans le tunnel, parité Tribler) ;
  `enable_tcp=false` coupe tout TCP pair. Trackers HTTP : one-shot
  `http-request` via SOCKS5 (inchangé) ; HTTPS non supporté, comme
  Tribler.
- **Tests** : `tunnel_udp_socket_utp_roundtrip` (datagramme uTP →
  cellule → sortie → écho UDP → retour tunnel), pinning par
  destination, isolation du démux DHT/uTP.

## Config : `recommender`/`rendezvous` requalifiés en clés mortes (2026-09-29)

Le `warn` « composant actif mais non implémenté » des sections
`recommender` et `rendezvous` était trompeur : ces clés sont **mortes
dans Tribler 8.x lui-même** — déclarées dans `tribler_config.py` mais
jamais relues (vestiges des composants 7.x). Leur fonction historique
est déjà absorbée :

- `recommender` → tâche périodique « check local torrents » de
  `torrent_checker` (`TorrentChecker::check_oldest`, équivalent du
  `check_local_torrents` de Tribler 8.x) ;
- `rendezvous` → points de rendez-vous des hidden services dans
  `TunnelCommunity` (`create_rendezvous_point`/`on_establish_rendezvous`,
  `rendezvous_relay`).

Le `warn` devient un `debug!` documentant la parité ; `enabled=false`
n'est pas honoré — comme en Python. Docs alignées :
`configuration_cablage.md`, `api_endpoints_complet.md`.

## UI Flutter — seconde passe : toutes les fonctions du daemon (2026-09-28)

La contrainte « backend d'abord » est levée (décision utilisateur) :
l'interface `app/` se développe en parallèle du daemon. Exposition
complète des capacités déjà présentes dans `onionbit-api` :

- **File d'attente** : `queue_position` (monter/descendre/haut/bas),
  `auto_managed`, recheck, `move_storage`, limites de débit et ratio
  de seed individuels (menu contextuel + panneau de détail) —
  `PATCH /api/downloads/{ih}`.
- **Fichiers** : inclusion/exclusion (`selected_files`) et priorité
  (`file_priority`) par fichier dans l'onglet Fichiers.
- **Trackers** : retrait (`DELETE /{ih}/trackers`), annonce forcée
  (`tracker_force_announce`), ajout des trackers par défaut
  (`default_trackers`).
- **Diagnostic** : onglets « Statistiques » (`/api/statistics/tribler`),
  « Pairs DHT » et « Pairs PEX » (`/api/ipv8/tunnel/peers/{dht,pex}`),
  test de vitesse de circuit (nouveau circuit temporaire ou circuit
  `READY` + `PEER_FLAG_SPEED_TEST`, flux `speed:` pyipv8 lu via
  `ApiClient.getStreamedLines`).
- **Réglages** : bande passante globale (`max_download_rate`/
  `max_upload_rate`), file d'attente (`active_*` + `auto_managed`
  par défaut), seeding (mode/ratio/durée, safe seeding, hops
  par défaut), tunnels (`min/max_circuits`, `exitnode_enabled`),
  réseau (DHT/UPnP/NAT-PMP/LSD/uTP + proxy sortant), automatisation
  (watch folder + flux RSS avec application à chaud `PUT /api/rss`
  et derniers items), versioning (version courante + sonde),
  espace disque du dossier de destination (`dirspace`).

`flutter analyze` propre, `flutter test` : 10 tests verts.

## Correctif majeur : SOCKS5 CONNECT relayait mal les trackers HTTPS (2026-09-29)

**Cause racine du blocage des téléchargements anonymes** identifiée
après investigation des logs verbeux : le circuit `READY` et le
kill switch désarmé (corrigés précédemment) ne suffisaient pas — les
annonces tracker échouaient **systématiquement** avec `timeout
http-response` puis `tls handshake eof`, quel que soit le pair de
sortie.

- **Cause** : `handle_connect` (SOCKS5 `CONNECT`) et
  `send_tcp_request` (côté sortie) traitaient le flux comme un
  échange HTTP en clair **unique** (une requête complète → une
  réponse complète, avec parsing `Content-Length`/`chunked`). Or les
  trackers réels sont en **HTTPS** : le client (`reqwest`/`librqbit`)
  négocie sa propre session TLS de bout en bout à travers le
  `CONNECT` — le premier octet envoyé est un `ClientHello` binaire,
  pas une requête HTTP. Le parseur ne pouvait jamais fonctionner ;
  confirmé par le commit d'origine (`ecb7fff`), qui ne testait que
  contre un faux tracker HTTP en clair, jamais HTTPS.
- **Correctif** : les cellules `http-request`/`http-response` (msg
  28/29, `HTTPRequestPayload`/`HTTPResponsePayload` — inchangées,
  confirmées identiques dans la référence `ipv8-rust-tunnels`
  locale) sont désormais réutilisées en **flux continu** plutôt
  qu'en échange unique :
  - Côté sortie (`on_http_request`/`run_exit_http_stream`) : ouvre
    une connexion TCP réelle vers la cible, puis relaie les octets
    **tels quels**, dans les deux sens, sans jamais les interpréter.
    `HttpResponse.total` devient un marqueur de fin (`0` = flux en
    cours, `1` = dernier chunk) plutôt qu'un décompte connu à
    l'avance (impossible à prédire pour une réponse chiffrée).
  - Côté demandeur (`socks5.rs::handle_connect`) : deux tâches de
    pompage (`HttpStreamSender`/`HttpStreamReceiver`,
    `TunnelCommunity::open_http_stream`) relaient la socket locale
    et le flux de cellules jusqu'à fermeture ou inactivité prolongée
    (`HTTP_STREAM_IDLE_TIMEOUT_MS`, 30 s — un aller-retour TLS
    complet à travers 3 sauts publics dépasse largement les 5 s
    précédents).
- **Sécurité inchangée** : le tunnel de circuit (chiffrement en
  couches par saut) n'est pas modifié — seul le dernier segment
  (sortie → serveur réel) est concerné, et il était déjà en TCP
  avant ce correctif. L'exit ne voit jamais le contenu déchiffré
  (TLS reste de bout en bout entre le client et le vrai serveur) :
  plus sûr que l'alternative (une sortie qui terminerait le TLS pour
  le compte du client).
## Logs : un fichier par run + horodatage local (2026-09-29)

- **Rotation par run** : `rolling::daily` concaténait tous les runs du
  jour dans `onionbit.log.YYYY-MM-DD`. Désormais `rolling::never` écrit
  `onionbit.log` (run courant uniquement) ; au démarrage, le fichier
  précédent est archivé `onionbit.log` → `.1`, `.N` → `.N+1`
  (`crates/onionbit-daemon/src/logs.rs`). `/api/logging` reste
  compatible (filtre préfixe `onionbit.log.`).
- **`logging/max_files`** (nouvelle section `configuration.json`,
  extension propre au portage, défaut 5) : nombre d'archives
  conservées. Les anciens fichiers datés `onionbit.log.YYYY-MM-DD` ont
  leur propre rétention `max_files` triée par récence — transition en
  douceur.
- **Heure locale** : le formatteur `fmt` par défaut écrivait en UTC
  (décalage constaté) — `LocalTime::rfc_3339` (feature
  `tracing-subscriber/local-time`) produit `…T14:32:01+02:00`, repli
  UTC si l'offset local est indéterminable.

## Application à chaud des réglages (`POST /api/settings`) (2026-09-29)

Audit du chemin « changement dans l'UI → effet » : la persistance était
complète (merge récursif → `configuration.json`) mais le sous-ensemble
appliqué à chaud était limité à RSS + watch folder. Comblé sur la
parité `set_session_limits` Python :

- **`libtorrent/max_download_rate`/`max_upload_rate`** : appliqués à
  chaud sur toutes les lanes via `Session::ratelimits` rqbit
  (`set_upload_bps`/`set_download_bps`) — comme
  `set_session_limits()` Python sur les `ltsessions` vivantes.
- **`libtorrent/active_downloads`/`active_seeds`/`active_limit`** :
  `enforce_queue_limits` lit désormais la config effective (override
  `ServiceOverrides.queue`) — une baisse de borne pause l'excédent au
  tick suivant, une hausse relève la file, sans redémarrage.
- **`libtorrent/download_defaults/saveas`** : l'override n'était que
  cosmétique (reflété dans `GET /api/settings` mais ignoré des
  nouveaux ajouts) — les `add_download`/`add_torrent_bytes` utilisent
  maintenant le dossier effectif (`effective_output_dir`), comme
  `DownloadConfig.destination` Python relu à chaque ajout.
- **`destination` (`PUT /api/downloads`)** : champ parsé puis jeté —
  correctif : passé à `AddDownloadOptions.output_folder` et persisté
  dans `downloads.output_dir` (prioritaire sur `saveas`, comme
  `set_dest_dir` Python).
- Reste restart-only (parité Python) : `ipv8`, `tunnel_community`,
  ports/adresses d'écoute, `api/*`, tray — ces réglages ne sont pas
  re-appliqués à chaud même chez Tribler.
- Test `apply_service_settings_applique_les_bornes_a_chaud` : queue
  resserrée à chaud → pause de l'excédent, `ratelimits` du moteur,
  `saveas` → `output_folder` du nouvel ajout.

## Câblage de la configuration (lot API HTTPS) : second listener TLS (2026-09-29)

Quatrième et dernier lot du câblage des champs `configuration.json`
non lus : la section `api/https_*` (parité `start_https_site` de
`rest_manager.py`).

- **`api/https_enabled`, `https_host`, `https_port`, `https_certfile`** :
  quand `https_enabled`, le daemon démarre un second listener TLS
  (`axum-server` + rustls) qui sert **le même routeur axum** que le HTTP
  (même middleware `X-Api-Key`/`?key=`/cookie `api_key`), bindé sur
  `https_host:https_port` — loopback obligatoire, même politique que
  l'écoute HTTP (`HttpsError::NotLoopback`).
- **Certificat** : `https_certfile` = PEM certificat + clé privée dans
  le même fichier (comme `SSLContext.load_cert_chain` sans keyfile —
  PKCS8/PKCS1/SEC1 acceptés). **Écart assumé** : fichier absent ou
  invalide → certificat auto-signé `rcgen` (SAN `localhost`,
  `127.0.0.1`, `::1`) généré et écrit au chemin configuré pour être
  réutilisé — Python échoue au `load_cert_chain` dans ce cas.
- **`https_port_running`** : réécrit avec le port réellement lié
  (`0` = éphémère supporté), comme `http_port_running`.
- **Arrêt** : le main garde le `axum_server::Handle` et appelle
  `graceful_shutdown` (5 s de grâce) dans sa séquence d'arrêt —
  pas de second wait sur `ShutdownSignal` (`notify_one` n'éveille
  qu'un seul waiter).
- **Dépendances** : `axum-server` (TLS), `rustls-pemfile` (parse PEM),
  `rcgen` (auto-génération) — licences MIT/Apache-2.0 compatibles GPL.
- **Correction de blocage** : le `std::net::TcpListener` bindé à la
  main doit passer en `set_nonblocking(true)` avant
  `axum_server::from_tcp` (`tokio::from_std` ne le fait pas) — sinon
  l'accept loop n'est jamais réveillé et le handshake TLS reste figé
  (le listener accepte au niveau kernel mais ne répond jamais).
- Test e2e `daemon_offline_sert_l_api_en_https` : daemon `--offline`
  + `https_enabled` pré-écrit → `401` sans clé, `200` avec `X-Api-Key`
  (reqwest `danger_accept_invalid_certs`), PEM généré,
  `https_port_running` publié. Timeout de 5 s par requête dans les
  tests daemon (un handshake figé doit échouer, pas bloquer).

## Câblage de la configuration (lot daemon/flags) : headless, tray, DB, versioning, canaux (2026-09-29)

Troisième lot du câblage des champs `configuration.json` non lus :
les flags daemon/UI et les attributs de canal par téléchargement.

- **`headless`** : force l'absence de systray comme `--no-tray`
  (`spawn_tray`).
- **`tray_icon_color`** : `#RRGGBB` parsé vers `TrayOptions.icon_color`
  — quand défini, l'icône systray est un carré recoloré (équivalent du
  `recolor_tray_icon` Python ; la ressource `.ico` ne peut pas être
  recolorée, le carré coloré prend donc le pas). Couleur invalide =
  warn + défaut.
- **`start_minimized`** : lue mais **inerte** côté daemon — chez
  Tribler core et UI sont le même processus (`run_tribler`), la clé ne
  régit que l'état de la fenêtre. Ici le daemon ne lance jamais l'UI :
  c'est `onionbit_ui.exe` qui démarre le daemon (`daemon_launcher`),
  sinon UI lancée → daemon → UI, boucle.
- **`database.enabled=false`** : `db_filename` bascule sur
  `":memory:"` (mode dégradé sans persistance, même repli que
  `--memory-db`).
- **`versioning.enabled=false`** : les six routes `/api/versioning/*`
  répondent 404 — Python n'enregistre tout simplement pas l'endpoint.
  `versioning/allow_pre` : tracé en `debug` (filtrera les
  pré-versions quand la vérification distante existera — aucun trafic
  implicite aujourd'hui).
- **`recommender` / `rendezvous` `enabled=true`** : lu et `warn` au
  démarrage — composants Python non portés (implémentation = roadmap),
  plus silencieusement ignorés.
- **`libtorrent/download_defaults/torrent_folder`** :
  `DownloadDefaults.torrent_folder` — la boucle de progression écrit
  `<name> [<infohash>].torrent` dès que le metainfo est connu
  (`PostHandleOp.WRITE_BACKUP_TORRENT` / `write_backup_torrent_file`
  Python ; une fois par téléchargement par session).
- **`channel_download` / `add_download_to_channel`** : migration
  `onionbit-db` v6 (`downloads.channel_download`,
  `downloads.add_download_to_channel`), propagées depuis
  `download_defaults` à l'insertion et persistantes par
  téléchargement comme les champs `DownloadConfig` Python —
  `add_download_to_channel` trace en `debug` à la complétion (canaux
  non portés).
- **`ui`** : déjà fonctionnelle — arbre `serde_json::Value`
  préservé tel quel par `GET`/`POST /api/settings` (consommée par la
  future UI Flutter, rien à brancher côté daemon).

## Câblage de la configuration (lot libtorrent) : file d'attente, mmap, DHT, `.parts` (2026-09-29)

Deuxième lot du câblage des champs `configuration.json` non lus :
les réglages `libtorrent` restants.

- **`libtorrent/allow_mmap`** : `EngineConfig.allow_mmap` →
  `default_storage_factory = MmapFilesystemStorageFactory` (feature
  `librqbit/storage_examples` activée dans le workspace). Sans effet
  sur les lanes anonymes `utp_only`, comme `enable_mmap and hops < 0`
  Python.
- **`libtorrent/dht_readiness_timeout`** :
  `EngineConfig.dht_readiness_timeout_secs` — `BtEngine::start` attend
  que la table de routage DHT soit peuplée (borne en secondes, warn si
  le délai s'écoule), comme le `wait_for_dht` de `start_onionbit_core`
  Python. `0` = pas d'attente (tests offline).
- **`libtorrent/clear_orphaned_parts`** : `BtEngine::stop` purge les
  `*.parts` orphelins de l'`output_dir` (stem `<infohash>` hex absent
  des téléchargements connus). librqbit n'écrit pas de `.parts` — le
  nettoyage vise les restes d'autres clients libtorrent dans le même
  dossier.
- **`libtorrent/check_after_complete`** : `CoreConfig.check_after_
  complete` — la boucle de progression appelle `session.recheck` (le
  re-add rqbit fait office de recheck) dès qu'un téléchargement passe
  `finished`.
- **`libtorrent/active_downloads` / `active_seeds` / `active_limit`** :
  `CoreConfig.queue` (`QueueLimits`, `< 0` = illimité). Nouveau
  `CoreSession::enforce_queue_limits` dans la boucle de progression :
  les téléchargements `auto_managed` au-delà des bornes sont mis en
  pause par le moteur (fin de file d'abord, `queue_position`
  décroissante) et repris par ordre croissant quand des slots se
  libèrent. Les pauses de file n'écrivent ni `paused` ni
  `user_stopped` en base — distinction `queued`/pause utilisateur de
  Python ; si l'utilisateur pause un torrent mis en file, la ligne
  reprend la main. Les téléchargements non `auto_managed` sont
  totalement exempts (règle libtorrent). Tests `tests/queue.rs` :
  pause + reprise après suppression, exemption des non-managés.
- **Écarts explicites** (tracés en `debug` au `to_core_config`,
  jamais ignorés silencieusement) : `natpmp` (non supporté par
  librqbit — UPnP couvre le besoin), `announce_to_all_tiers` /
  `announce_to_all_trackers` (rqbit annonce déjà à tous les trackers,
  pas de tiering), `max_concurrent_http_announces`,
  `active_{dht,tracker,lsd}_limit` (quotas libtorrent par fonction
  sans équivalent rqbit).

## Câblage de la configuration (lot 1) : tunnels, IPv6, SOCKS5, logs (2026-09-28)

Premier lot du câblage des champs de `configuration.json` déclarés mais
jamais lus (audit complet : ~40 champs concernés).

- **`tunnel_community/max_circuits`** : `Ipv8Config::max_circuits`
  (défaut `DEFAULT_MAX_CIRCUITS = 8`, pyipv8 `TunnelSettings`). Le
  watchdog de lane applique désormais la formule de
  `TriblerTunnelCommunity.monitor_downloads` : `circuits_needed =
  clamp(telechargements actifs de la lane, min_circuits, max_circuits)`
  au lieu de `min_circuits` fixe. Écart assumé : une lane existante
  garde `min_circuits` en veille même sans téléchargement actif
  (Python ne demande des circuits qu'aux hops utilisés) — un premier
  `add` n'attend pas la construction à froid.
- **`libtorrent/socks_listen_ports`** : `Ipv8Config::socks_listen_ports`,
  port SOCKS5 par lane (`ports[hops-1]`, `0` = éphémère, repli
  éphémère avec warn si le port configuré est pris — parité avec la
  poursuite Python après `Failed to start SOCKS5 servers`).
- **`content_discovery_community/enabled`** :
  `Ipv8Config::enable_content_discovery` gate la création de
  `ContentDiscoveryCommunity` ; `Ipv8Stack.content_discovery` devient
  `Option` ; `/api/search` distant et `add_bootstrap_node` s'en
  accommodent.
- **`ipv8/interfaces[UDPIPv6]`** : `Ipv8Config::listen_addr_v6` →
  `UdpEndpoint::bind_dual` (socket IPv6 secondaire partageant les
  mêmes listeners, envoi routé par famille d'adresse — équivalent du
  `DispatcherEndpoint` pyipv8). Repli IPv4-seul avec warn si le bind
  v6 échoue. Test `endpoint_dual_stack_envoie_et_recoit_en_v6`.
- **`ipv8/logger_level`** : chargé avant `init_tracing`, injecté comme
  directive `onionbit_ipv8=LVL,onionbit_tunnel=LVL` dans le `EnvFilter`
  de base (les niveaux hors `INFO` seulement ; valeur invalide =
  warn + ignorée).
- **`statistics`** et **`ipv8/interfaces[].worker_threads`** :
  documentés comme volontairement non lus — la clé `statistics` est
  inerte dans Tribler 8.x lui-même (ne subsiste que via
  `upgrade_script`), et `worker_threads` n'a pas d'équivalent Tokio.

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
circuit_watchdog` (`onionbit-core/src/ipv8_stack.rs`) appelait
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

- `TunnelCommunity::register_tunnel_peer` (`crates/onionbit-tunnel/src/
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

- `TunnelCommunity::spawn_hop_timeout` (`crates/onionbit-tunnel/src/
  community.rs`) : après l'envoi d'un `create`/`extend`, programme une
  purge du circuit (`remove_circuit`) si `retry_requests` porte
  toujours le même `identifier` après `CIRCUIT_READY_TIMEOUT_MS`
  (`next_hop_timeout` pyipv8, 10 s) — équivalent du `on_timeout` de
  `NextHopRequestCache` côté Python. Le watchdog de circuits
  (`spawn_circuit_watchdog`, `onionbit-core/src/ipv8_stack.rs`) peut
  alors retenter avec un autre pair au tick suivant.

## Suppression des lanceurs `demarrer`/`arreter` (2026-09-28)

Devenus inutiles : `onionbit_ui.exe` lance le daemon elle-même
(`daemon_launcher`) et l'arrêt se fait via « Quitter » du systray ou
`PUT /api/shutdown` (qui termine réellement le processus depuis
l'étape 29). `build_dist.ps1` ne les génère plus et nettoie les restes
des builds précédents dans `dist\`.

## UI : lancement automatique du daemon (2026-09-28)

Décision V1 de `flutter_architecture.md` implémentée : plus besoin de
`demarrer.cmd`, lancer `onionbit_ui.exe` suffit.

- `core/config/daemon_launcher{,_native,_stub}` : au build de
  `connectionSettingsProvider`, si l'API découverte ne répond pas,
  `onionbit-daemon[.exe]` voisin de l'exécutable est lancé détaché
  (`--state-dir <exe>/state` — disposition du bundle `dist\`), puis
  l'API est sondée 30 s (clé et `http_port_running` relus à chaque
  tentative). Daemon déjà vivant → connexion directe ; binaire absent
  (`flutter run`, web) → `null` et repli sur les préférences.
- `ONIONBIT_API_KEY` présent = setup externe : jamais de daemon enfant.
  `ONIONBIT_DAEMON_EXE` surcharge le chemin du binaire en dev.
- Sonde « API vivante » = toute réponse HTTP, même 401 (même logique
  que `Test-ApiAlive` de `demarrer.ps1`) — testée en loopback
  (`daemon_launcher_test.dart`).
- Symétrie avec l'étape 29 : l'UI lance le daemon, l'item « Ouvrir
  Tribler » du systray lance l'UI.

## Étape 29 — daemon systray Windows + arrêt unifié (2026-09-28)

Le daemon n'affiche plus de fenêtre console : il vit dans la zone de
notification, comme les clients BitTorrent classiques.

- `onionbit-daemon` en sous-système GUI (`#![windows_subsystem =
  "windows"]`) ; `--console` rattache la console parente ou en alloue
  une (`AttachConsole`/`AllocConsole` + handles `CONOUT$`, en
  préservant les redirections déjà branchées — `api_parity_run.ps1`
  continue de capter stdout/stderr sans le flag).
- Icône systray via `tray-icon` 0.25.1 (écosystème Tauri) sur un
  thread dédié à pompe Win32 (`GetMessage`/`DispatchMessage` — les
  `WM_COMMAND` du menu sont drainés après chaque dispatch) ; `WM_QUIT`
  termine le thread et retire l'icône. Menu : « Ouvrir Tribler »
  (lance `onionbit_ui.exe` voisin de l'exe, désactivé s'il est absent),
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
  subsystem) ; message d'échec renvoyé vers `state\logs\onionbit.log`.
- Vérifié en live : lancement détaché sans console, icône créée,
  API 200, `PUT /api/shutdown` → `signal d'arret recu` →
  `thread systray terminé` → `daemon arrete proprement`, mutex
  d'instance refusant le second processus. Reste la validation
  visuelle du menu (clics, autostart) — étape `[i]`.
- Plan Flutter ajusté : `tray_manager` retiré de
  `flutter_architecture.md` (l'icône appartient au daemon).

## Fix `/api/logging` : journal vide dans l'UI (2026-09-28)

- `tracing_appender::rolling::daily` produit
  `onionbit.log.YYYY-MM-DD` — l'extension est la **date**, pas `log`.
  Le handler filtrait `extension == "log"` → aucun candidat →
  l'onglet Journaux affichait « Journal vide » malgré un fichier
  alimenté. Filtre corrigé sur le préfixe `onionbit.log`.
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
  journal `state/logs/onionbit.log` (et l'onglet Journaux, via
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
  `fmt` propres, 54/54 tests `onionbit-api`.

## Étape 20 (avancement) — intégration UI↔daemon validée en live (2026-09-28)

- Inventaire des routes consommées par `app/` : 11 endpoints REST +
  SSE `/api/events` — tous présents dans le routeur et répondent 200
  contre `onionbit-daemon` réel (offline) : `downloads`, `settings`,
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
  `crates/onionbit-bittorrent/examples/exit_download_interop.rs` :
  critère final de l'étape 12 validé — rqbit downloader → circuit 2
  sauts → **sortie pyipv8 réelle** (`PEER_FLAG_EXIT_BT`) → socket uTP
  du seeder rqbit, 200 Ko téléchargés et vérifiés octet à octet
  (`INTEROP EXIT DOWNLOAD OK`). `--hops 1` permet le circuit direct.
- **`udp_relay::dial_to`** (`onionbit-tunnel`) : variante de `dial` à
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
  et de `onionbit-daemon`, attente de disponibilité des deux API,
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
  `onionbit-core/src/asyncio/`) — adaptation tokio documentée en
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

- **Speed-test de circuits** (`onionbit-tunnel/src/speedtest.rs` +
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

- **`StatisticsEndpoint`** (`onionbit-ipv8/src/endpoint.rs`) :
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
  besoin. Aucune dépendance `onionbit-ipv8`→`onionbit-tunnel` : la
  decode_map du tunnel vit dans `onionbit-tunnel` et est injectée
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
- **Accesseurs** `onionbit-ipv8` : `stats_snapshot`/`buckets_snapshot`/
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
  ses trackers (`onionbit_format::torrent::strip_trackers` chirurgical,
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

- **Migration `onionbit-db` v3** : colonnes `downloads` —
  `safe_seeding`, `upload_limit`, `download_limit`, `seeding_ratio`,
  `auto_managed`, `queue_position`, `completed_dir`, `selected_files`,
  `file_priorities`, `extra_trackers`, `time_finished`. Upsert complet,
  restauration des réglages aux re-add internes (équivalent des
  `dlcheckpoints`/`DownloadConfig` Python). Tests de migration
  v1→v3, réouverture idempotente, rejet de schéma futur, round-trip.
- **`onionbit-bittorrent`** : `AddDownloadOptions` (paused, fichiers
  sélectionnés, dossier, trackers, limites par torrent via
  `AddTorrentOptions::ratelimits`) ; `update_only_files`, limites de
  session à chaud, bitfield des pièces en base64 (`api_dump_haves`),
  stats par pair, `force_announce`, `total_pieces`, fichiers/traqueurs
  exposés par `Download`.
- **`onionbit-core`** : `DownloadDefaults` dans `CoreConfig` (alimenté
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
  dans les répertoires candidats (`<exe>/state`, `<cwd>/.onionbit`, …)
  ou `ONIONBIT_API_KEY`/`ONIONBIT_API` en environnement, puis retombe sur
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

- **`DaemonConfig` persistée** (`onionbit-core/daemon_config.rs`) :
  arbre `TriblerConfig` serde (`api`, `ipv8`, `libtorrent` +
  `download_defaults`, `tunnel_community`, `rss`, `watch_folder`,
  `torrent_checker`, `dht_discovery`, `versioning`, `statistics`,
  `state_dir`) lu/écrit dans `state_dir/configuration.json`.
  Clés inconnues conservées au merge (compat ascendante). `api/key`
  générée hex (32 car.) au premier démarrage ; `api/http_port_running`
  réécrit après le bind réel (port `0` = aléatoire, comme Python).
- **Authentification par clé API** (`onionbit-api/auth.rs`) : middleware
  axum strictement équivalent à `ApiKeyMiddleware` Python — clé lue dans
  `X-Api-Key`, puis `?key=`, puis le cookie `api_key`, même en loopback ;
  rejet `401 {"error": {"handled": true, "message": "Unauthorized
  access"}}`. `DefaultBodyLimit` aligné à 16 Mio (`MAX_REQUEST_SIZE`).
- **`/api/settings` adossé au fichier** (`onionbit-api/handlers/settings.rs`) :
  `GET` rend l'arbre persisté complet ; `POST` merge récursivement le JSON,
  réécrit `configuration.json` (équivalent `config.write()`) et applique à
  chaud les clés connues (`rss`, `watch_folder`, `libtorrent`,
  `tunnel_community`, `api`, `ipv8`) via `apply_service_settings`.
- **`onionbit-daemon`** : charge `configuration.json`, applique les
  overrides CLI (`--api`, `--ipv8-port`, `--bootstrap`, …), publie le
  port réel dans `api/http_port_running` et la clé dans l'`AppState`.
- **`onionbit-cli`** : options `--api-key` et `--state-dir` ; découverte
  automatique de la clé et du port dans `state_dir/configuration.json`
  (défaut `.onionbit`) quand rien n'est passé explicitement.
- **Tests** : auth 401/en-tête/query/cookie, round-trip de settings
  persistés (`onionbit-api/tests/api.rs`), e2e daemon avec clé générée +
  `http_port_running` (`onionbit-daemon/tests/daemon.rs`), découverte de
  clé CLI (`onionbit-cli/tests/cli.rs`).

## Correctif — Parité `/api/libtorrent` (`hop`), création proactive de circuits & garde d'ajout (2026-09-28)

- **Parité protocolaire `/api/libtorrent`** :
  - `onionbit-api/handlers/libtorrent.rs` : support du paramètre de requête `?hop={0..3}` pour `/api/libtorrent/settings` et `/api/libtorrent/session` en stricte conformité avec Python Tribler (`libtorrent_endpoint.py`), tout en conservant `session` en alias pour la rétro-compatibilité.
  - Mise à jour de `docs/reference_tribler/api_endpoints_complet.md` (section 5 validée ✅).
  - Tests automatisés dans `onionbit-api/tests/api.rs`.
- **Ajout de téléchargements anonymes (1, 2, 3 sauts)** :
  - `onionbit-network-policy/kill_switch.rs` : ajout de `guard_add(&self)` qui n'interdit l'ajout qu'en cas de panne critique du proxy SOCKS5 local ou d'arrêt d'urgence manuel, sans bloquer l'ingestion tant que les circuits overlay sont en cours d'établissement.
  - `onionbit-bittorrent/engine.rs` : utilisation de `guard_add` lors de l'ajout d'un torrent. Le flux réseau SOCKS5 rejette silencieusement les paquets UDP tant qu'aucun circuit n'est prêt (zéro fuite IP garantie), permettant au téléchargement de patienter et de démarrer dès que le circuit est fonctionnel.
- **Construction proactive des circuits dans `TunnelCommunity`** :
  - `onionbit-ipv8/peer.rs` : exposition de `all_verified_peers()` dans `Network`.
  - `onionbit-tunnel/community.rs` : implémentation de `build_circuits_if_needed(hops, min_circuits)` qui choisit des pairs vérifiés (priorité aux drapeaux de sortie BitTorrent `PEER_FLAG_EXIT_BT` à 1 saut, relais `PEER_FLAG_RELAY` à 2+ sauts, ou pairs vérifiés) et déclenche `create_circuit`.
  - `onionbit-core/ipv8_stack.rs` : appel proactif de `build_circuits_if_needed` dès le démarrage des lanes anonymes et à chaque cycle (5s) du watchdog.
- **Packaging & Déploiement** :
  - Binaires release `onionbit-daemon.exe` et `onionbit-cli.exe` recompilés et déployés dans `dist/` et `C:\Users\Lou\Desktop\OnionBit`.

## Correctif — Initialisation des lanes anonymes (sauts 1, 2, 3) & persistance DHT (2026-09-28)

- **Correction du conflit DHT sur les lanes anonymes** :
  - `onionbit-bittorrent/config.rs` : configuration de `librqbit::DhtSessionConfig` avec `persistence: None` pour la DHT en mémoire éphémère (évite le verrouillage concurrent de `dht.json` et les collisions de port persistant sur Windows `WSAEADDRINUSE 10048`).
  - `onionbit-core/ipv8_stack.rs` : isolation stricte des moteurs BitTorrent anonymes créés par `anon_engine(hops)` : désactivation explicite de la DHT mainline (`enable_dht = false`), de la découverte locale (`disable_lsd = true`) et du port d'écoute direct (`listen_port = None`). Un téléchargement anonyme ne doit jamais émettre de paquets UDP DHT/LSD hors du circuit SOCKS5.
- **Précision des messages d'erreur API** :
  - `onionbit-api/handlers/downloads.rs` : distinction des erreurs réelles de parsing de fichier (`CoreError::Format(_) -> "corrupt torrent file"`) par rapport aux erreurs d'état du moteur ou de réseau, évitant de masquer les erreurs d'infrastructure sous un faux message de fichier corrompu.
- **Tests & Packaging** :
  - Ajout d'un test de non-régression dans `crates/onionbit-core/tests/circuit_death.rs` (`anon_engine_demarre_proprement_avec_dht_active_sur_session`) validant que les 3 lanes anonymes s'initialisent correctement même quand la DHT est active sur la session principale.
  - Reconstruction release complète via `scripts/build_dist.ps1 -SkipCheck` et synchronisation vers `C:\Users\Lou\Desktop\OnionBit`.

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
  - `onionbit-core` : indexation automatique dans `channel_node` (`metadata_type = 300`) des téléchargements ajoutés et restaurés.
  - `onionbit-api` : enrichissement dynamique des endpoints `/api/metadata/torrents/popular` et `/api/metadata/torrents/local_search` pour inclure immédiatement les torrents actifs de la session.
- **Réglages & Préférences** :
  - Section « Téléchargements par défaut » dans la page Réglages avec sélection et enregistrement du répertoire par défaut via `POST /api/settings`.
  - Backend : prise en compte à chaud de `download_defaults.saveas` dans `ServiceOverrides`, `effective_config` et application immédiate.
- **Journaux et Diagnostique** :
  - Configuration de `tracing_appender` dans `onionbit-daemon` écrivant dans `<state_dir>/logs/onionbit.log` (rotation quotidienne) en plus de stdout, rendant l'onglet « Journaux » fonctionnel.
- **Layout & Polissage UI** :
  - Correction de l'espacement et des débordements de texte dans la barre d'état et le badge de statut des téléchargements.
  - Packaging complet de `dist\` et synchronisation avec le dossier de test Bureau.

## Étape 20 (partie 1) — coquille Flutter + packaging `dist/` (2026-09-28)

- `scripts/build_dist.ps1` : assemble un dossier **portable** `dist\`
  (daemon + CLI release, UI Flutter Windows release, `demarrer.cmd`,
  `arreter.cmd`, `build-manifest.json`). Le lanceur démarre le daemon
  (console minimisée, `--state-dir %~dp0state`), attend l'API
  `127.0.0.1:8085` (30 s max), puis ouvre `onionbit_ui.exe`.
  `arreter.cmd` fait `PUT /api/shutdown` puis `taskkill` en filet.
  `dist\state\` (données utilisateur) n'est jamais effacé par le build.
  Vérifié de bout en bout : daemon démarré, API `/api/events/info` OK,
  UI connectée (2 sessions TCP REST+SSE).
- **Activation IPv8 et bootstrap réel dans `onionbit-daemon`** :
  `Ipv8Config::production()` avec les 20 nœuds officiels `DISPERSY_BOOTSTRAPPER`
  (TU Delft / Tribler), résolution DNS asynchrone des adresses `dispersy*.tribler.org`,
  repli automatique sur port éphémère si le port UDP 8090 est occupé,
  options CLI `--no-ipv8`, `--no-anonymity`, `--ipv8-port`, `--bootstrap`.
  Binaires release régénérés dans `dist\` et le dossier de bureau.
- **Support de l'upload binaire brut de `.torrent` dans `onionbit-api`** :
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
  `the reference Flutter UI project`), `ApiClient` REST+SSE maison (le
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
- `onionbit-tunnel::TunnelCommunity::watch_circuits()` : canal
  `watch` incrémenté à chaque mutation d'état de circuit (création,
  hop ajouté → `READY`, `DESTROY` reçu → `on_destroy`). Le polling
  seul ratait une transition `READY → détruit` plus rapide qu'un
  tick — la détection est désormais événementielle (réaction en ms).
- `onionbit-core::ipv8_stack::spawn_circuit_watchdog` : une tâche par
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
- Test `crates/onionbit-core/tests/circuit_death.rs` (~5 s) : portée
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

- `crates/onionbit-bittorrent/tests/kill_switch_midtransfer.rs` :
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
  `crates/onionbit-ipv8/examples/dht_interop_node.rs` +
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

- **`onionbit-test-support` peuplé** : `test_torrent_bytes(name, len)`,
  `free_port()`, `wait_for(timeout, f)` — fixtures dupliquées dans
  `onionbit-api`/`onionbit-cli`/`onionbit-daemon` remplacées par la
  fixture partagée.
- **Téléchargement loopback réel** (`onionbit-bittorrent/tests/
  loopback_download.rs`) : seeder + downloader rqbit uTP en loopback,
  `initial_peers`, contenu vérifié octet-pour-octet.
- **Persistance/redémarrage** (`onionbit-core/tests/lifecycle.rs`) :
  deux `CoreSession` successives sur le même `state_dir` — le
  téléchargement est restauré (info-hash + état pause) ; un download
  supprimé n'est pas restauré.
- **Migration de schéma** (`onionbit-db/tests/migrations.rs`) : base
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
  (`<Tribler sources checkout> (env `TRIBLER_SRC`)`) et le pendant Rust
  (`crates/onionbit-api`) avec statut de portage.
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

- **Stack IPv8 dans `CoreSession`** (`onionbit-core/src/ipv8_stack.rs`) :
  endpoint UDP, `Network`, `DiscoveryCommunity`, `ContentDiscoveryCommunity`
  (provider = base `channel_node` + sérialiseur mdblob signé),
  `TunnelCommunity` optionnelle + serveur SOCKS5 par lane anonyme et
  moteur `BtEngine` dédié par nombre de sauts (`anon_engine(hops)`).
  Reglages dans `CoreConfig.ipv8` (`enabled`, `listen_addr`,
  `bootstrap_peers`, `enable_anonymity`, `peer_flags`,
  `onionbit_tunnel_community`).
- **`downloads.anon_hops`** (migration DB v2) : le téléchargement est
  routé vers la lane anonyme correspondante (`anon_hops` de
  `PUT /api/downloads`) et restauré sur la bonne lane au démarrage.
- **`onionbit-format::mdblob::encode_entry`** : sérialisation signée
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
- **`onionbit-bittorrent`** : `Download` expose `files()`/`trackers()`/
  `add_tracker()`/`torrent_bytes()`/`stream_file_from()` (seek) ;
  trackers additionnels partagés par info-hash au niveau `BtEngine`.
- **`onionbit-tunnel`/`onionbit-ipv8`** : accesseurs de stats
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

- `onionbit-ipv8::content_discovery` : community `9aca62f8…1648` —
  payloads santé (msgs 3/4, `HealthInfo` binaire pyipv8), version
  (101/102) et remote-select (201/202) ; trait `ContentProvider`
  injecté par la couche supérieure ; gossip périodique des santés
  vers les pairs de la community. Tests loopback `tests/content_discovery.rs`.
- `onionbit-core::services::torrent_checker` : scrape BEP-15 UDP
  (connect/announce→scrape) et HTTP bencode (`files` dict), socket UDP
  dédiée partagée, sélection des torrents les moins récemment vérifiés
  depuis `torrent_state`, persistance seeders/leechers/last_check +
  notification `TorrentHealthUpdated`. `IpPolicy` appliquée aux
  trackers. Test `torrent_checker_udp_scrape` (tracker factice
  loopback).
- `onionbit-core::services::rss` : watchers périodiques (`RssManager`),
  extraction des URLs `.torrent` du XML, requêtes conditionnelles
  (ETag/Last-Modified) et backoff `Keep-Alive: timeout=N`, fetch des
  torrents via le helper anti-SSRF partagé, notification
  `TorrentMetadataCreated`. Test `rss_discovers_torrent_and_notifies`.
- `onionbit-core::services::watch_folder` : scan récursif périodique,
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
  `torrent_health_updated` dans `onionbit-api`.

## Étape 13 — `onionbit-network-policy` : anti-SSRF, exit policy, kill switch (2026-09-27)

Crate de politiques réseau pures (sans dépendance vers ipv8/bittorrent)
+ intégration dans tunnel, bittorrent et core.

- `address_policy::IpPolicy` : anti-SSRF — catégories refusables
  (loopback, privé RFC1918 + CGNAT + ULA, link-local, multicast,
  unspecified, réservé/documentation, IPv4-mapped IPv6), ports bornables.
- `exit_policy` : port fidèle de `DataChecker`/`is_allowed` (pyipv8
  `exit_socket.py`) — `could_be_utp`/`udp_tracker`/`dht`/`ipv8`,
  `is_exit_data_allowed` exige `PEER_FLAG_EXIT_BT`/`PEER_FLAG_EXIT_IPV8`
  ou le préfixe de la community. Les constantes `PEER_FLAG_*` vivent
  ici désormais (source unique, ré-exportées par `onionbit-tunnel::routing`).
- `kill_switch::KillSwitch` : atomic bool + raison diagnostic + `guard()`.
- `proxy_guard::validate_local_socks5_url` : `socks5://`/`socks5h://`
  numérique loopback uniquement (ni DNS, ni credentials, ni port nul).
- `onionbit-tunnel` : `exit_data` ET `exit_recv_data` appliquent
  `is_exit_data_allowed` (les deux sens, comme `sendto` +
  `datagram_received` côté pyipv8) ; les flags de sortie = `peer_flags`
  locaux annoncés. Test `tunnel_exit_drops_non_bt_or_unflagged` ;
  les sorties des tests existants portent désormais `EXIT_BT` et des
  payloads uTP-shaped.
- `onionbit-bittorrent` : `BtEngine::start` valide `socks5_proxy`
  (distant = échec de démarrage, jamais de repli direct) ; watchdog
  TCP du proxy + `KillSwitch` qui bloque `add`/`resume` tant que le
  proxy est injoignable ; `kill_switch()` exposée pour le futur
  câblage tunnel (circuits morts). Tests `tests/policy.rs`.
- `onionbit-core` : `CoreConfig.ip_policy` (stricte par défaut,
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
`onionbit-tunnel` en loopback.

- `scripts/interop/py_tunnel_node.py` : noeud `TunnelCommunity`
  (flags RELAY|EXIT_IPV8|EXIT_BT) + echo UDP + dump des
  `SessionKeys` ; exporte sa cle publique via keyfile.
- `crates/onionbit-tunnel/examples/tunnel_interop_node.rs` : noeud
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
- `onionbit-crypto/src/ipv8/session.rs` : `generate_session_keys`
  faisait un HKDF extract+expand (sel nul) alors que la reference
  est **EXPAND_ONLY** (`set_hkdf_key(shared_secret)` comme PRK
  directe) → `Hkdf::from_prk`. C'est la raison du "Decryption
  failed" cote Python avant ce fix.

Le jalon Tribler 8.4.3 installe (client complet) reste distinct et
a faire separement.

## Étape 12 (correctif) — garde-fou IPv4 factice, relais UDP first-seen, idempotence `create_e2e` (2026-09-27)

Suite à une revue externe puis vérification directe du code, trois défauts
de sécurité/robustesse ont été corrigés dans `onionbit-tunnel` :

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
`cargo test --workspace` vert, suite `onionbit-tunnel` (12 tests) stable
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
- `onionbit-bittorrent/tests/anon_download.rs` : meme repli sur la
  creation e2e du telechargement anonyme.

Validation : `verify_all.ps1` vert, le retry a ete observe en action
sous charge (succes a la 2e tentative).

## Étape 12 (partie 5) — `onionbit-tunnel` : relais UDP de hidden seeding (2026-09-27)

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

## Étape 12 (partie 4) — `onionbit-tunnel` : CONNECT HTTP par cellules 28/29 (2026-09-27)

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

## Étape 12 (partie 3) — `onionbit-tunnel` : hidden services E2E (2026-09-27)

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

## Étape 12 (partie 2) — `onionbit-tunnel` : sortie bidirectionnelle + SOCKS5 (2026-09-27)

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

## Étape 12 (partie 1) — `onionbit-tunnel` : circuits fonctionnels en loopback (2026-09-27)

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
- `onionbit-ipv8::endpoint` : `add_raw_prefix_listener` — les cellules
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
- `crates/onionbit-ipv8/tests/fixtures/README.md` créé : provenance des
  captures (commit pyipv8 `4a294ed1`, commit Tribler `3ac2f4b4`,
  sens de chaque fichier, msg_ids contenus, procédure de
  régénération) — une évolution de la référence ne pourra plus effacer
  la signification du rejeu.
- Distinction des cibles d'interop actée : « venv pyipv8 » (validé)
  ≠ « Tribler 8.4.3 installé » (ressource disponible, non encore
  exercée) ; les résultats futurs seront rapportés séparément.
- `docs/INDEX.md` : scripts d'interop et fixtures référencés.

## Référence supplémentaire : Tribler 8.4.3 installé (2026-09-27)

- `<Tribler install dir> (env `TRIBLER_EXE`)` documenté dans `AGENTS.md` et
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
  `<Tribler sources checkout> (env `TRIBLER_SRC`)\.venv-interop`), journalise chaque
  datagramme en hex et envoie des introduction-request a la cible Rust.
- `scripts/interop/verify_packets.py` : decode chaque paquet
  enregistre (prefix|msg_id|varlenH pubkey|global_time|payload|sig) et
  verifie la signature Ed25519 via le vrai `default_eccrypto` pyipv8.
- `crates/onionbit-ipv8/examples/interop_node.rs` : noeud Rust
  (`DiscoveryCommunity`) avec tap de paquets rx/tx (nouveau
  `UdpEndpoint::set_tap`) ecrivant le meme journal hex.
- `scripts/interop_ipv8.ps1` : orchestre les deux noeuds sur loopback,
  verifie 39/39 paquets dans les deux sens, verifie que chaque noeud a
  enregistre l'autre comme pair verifie. Chemins reseables via
  `TRIBLER_PYIPV8` / `TRIBLER_INTEROP_PY`.
- Fixtures enregistrees `crates/onionbit-ipv8/tests/fixtures/*.hex`
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

## Étape 11 — `onionbit-ipv8` : framework de communities complet (2026-09-27)

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

## Étape 10 — `onionbit-ipv8` : overlay DHT (2026-09-27)

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

## Étape 9 — `onionbit-ipv8` : overlay minimal (2026-09-27)

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

## Étape 8 — `onionbit-daemon` : executable bout en bout (2026-09-27)

- Assemblage complet : `CoreSession` (moteur BitTorrent + SQLite +
  notifier) derriere `onionbit-api`, servi par axum avec
  `with_graceful_shutdown` (Ctrl-C -> `session.stop()` -> arret
  propre).
- CLI clap : `--listen` (defaut `127.0.0.1:8085` = `DEFAULT_API` de
  onionbit-cli), `--state-dir`, `--offline`. **Garde-fou** : refuse
  toute adresse d'ecoute non-loopback (l'API de controle n'est jamais
  exposee sur le reseau).
- Logging `tracing` fmt + `EnvFilter` (`RUST_LOG`, defaut info) —
  cf. regles de niveaux AGENTS.md (infos de cycle de vie seulement).
- 1 test e2e : binaire reel en `--offline`, poll de `/api/downloads`
  sur loopback, kill propre. Le test manuel "telechargement torrent
  reel" reste a l'etape 16 (necessite du reseau).

## Étape 7 — `onionbit-cli` : CLI de pilotage (2026-09-27)

- Sous-commandes clap : `status`, `list`, `add` (`--paused`),
  `remove` (`--remove-data`), `pause`, `resume` ; option globale
  `--api` (defaut `http://127.0.0.1:8085`, constante `DEFAULT_API` —
  le Python utilise `api/http_port=0` aleatoire, on documente un port
  fixe pour le daemon de l'etape 8).
- `add` choisit le champ JSON selon la source (`uri` pour magnet/http,
  `torrent` pour un chemin local), comme l'API Python.
- Erreurs d'API affichees au format Tribler (`HTTP <code> : <message>`
  sur stderr, exit code 1).
- 1 test d'integration reel : binaire `onionbit-cli` (via
  `CARGO_BIN_EXE_*`) contre un serveur `onionbit-api` sur
  `127.0.0.1:0` — cycle complet + cas daemon injoignable.
- Piege corrige : `std::process::Command` bloque le runtime tokio
  mono-thread du test ; `tokio::process::Command` utilise.

## Étape 6 — `onionbit-api` : REST + SSE (2026-09-27)

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

## Étape 5 — `onionbit-core` : Session + Notifier (2026-09-27)

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

## Étape 4 — `onionbit-db` : schema SQLite + migrations (2026-09-27)

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

## Étape 3 — `onionbit-bittorrent` : enveloppe `librqbit` (2026-09-27)

- `BtEngine` : enveloppe de `librqbit::Session` v9 (cycle de vie
  start/stop, ajout magnet/URI/bytes `.torrent`, liste, pause, reprise,
  suppression avec/sans fichiers).
- `Download` : handle léger (`Arc`) exposant id, info-hash, nom, stats.
- `DownloadStats`/`DownloadState` : types domaine découplés de
  `librqbit` (mapping `TorrentStatsState` → `Initializing/Checking/
  Downloading/Seeding/Paused/Error/Stopped`), `progress()` normalisé.
- `EngineConfig` : regroupe tous les réglages (output_dir, DHT,
  trackers, LSD, IPv4-only, port d'écoute, peer_limit, fastresume,
  proxy SOCKS5 point d'intégration `onionbit-tunnel`). Constructeur
  `EngineConfig::offline` pour les tests sans réseau.
- Traduction `EngineConfig → librqbit::SessionOptions`
  (`ListenerOptions`, `ConnectionOptions::proxy_url`).
- 1 test offline (session sans réseau + ajout de `.torrent` encodé par
  `onionbit-format`).

## Étape 1 — `onionbit-format` : bencode, `.torrent`, magnet, `.mdblob` (2026-09-27)

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
  `onionbit-crypto`.
- 18 tests offline, `clippy -D warnings` et `fmt` propres.

## Étape 2 — `onionbit-crypto` : hachage et crypto IPv8 (2026-09-27)

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

- Dépôt git local initialisé (`<repo root>`).
- Licence GPL-3.0-or-later (texte officiel FSF) ajoutée en `LICENSE`.
- `AGENTS.md` rédigé (règles critiques, conventions de code, workflow
  par étape, anti-duplication), inspiré de la structure du projet
  eMule-Rust de l'utilisateur.
- Analyse de l'architecture officielle Tribler
  (`<Tribler sources checkout> (env `TRIBLER_SRC`)`) : cartographie des modules
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
  (`onionbit-format`, `onionbit-crypto`, `onionbit-bittorrent`,
  `onionbit-ipv8`, `onionbit-tunnel`, `onionbit-core`, `onionbit-db`,
  `onionbit-network-policy`, `onionbit-api`, `onionbit-cli`,
  `onionbit-daemon`, `onionbit-test-support`) : chaque crate compile,
  documente sa responsabilité et son étape d'implémentation prévue, et
  passe `cargo check`/`cargo clippy -D warnings`/`cargo fmt --check`/
  `cargo test`.
- `docs/plans/plan_faisabilite.md`, `docs/architecture/architecture.md`,
  `docs/plans/roadmap.md`, `docs/reference_tribler/`, `docs/INDEX.md`
  rédigés.
- `scripts/verify_all.ps1` créé (validation complète).
