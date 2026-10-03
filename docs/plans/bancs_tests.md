# Catalogue des bancs de test — OnionBit

Catalogue **reproductible** des tests sur banc (loopback multi-nœuds,
interop pyipv8, Tribler.exe installé, réseau public, sécurité, produit).
Ce n'est pas un inventaire exhaustif figé : chaque entrée possède un ID
stable, un niveau de priorité, un script, un oracle observable et des
artefacts attendus, afin qu'un run puisse signaler précisément **quel
changement a régressé quelle garantie**.

Commit de référence de la présente matrice : `60342bc` + `920501e`
(durcissement PING/PONG, DHT anonyme, guards par défaut).

## Règles d'exécution

- **Séquentialité entre niveaux** : archiver les artefacts et arrêter
  tous les processus entre deux niveaux. Ne jamais faire tourner en
  concurrence banc public Tribler, fuzzing et mesh de fingerprinting
  sur la même machine (famine d'API Tribler observée → faux
  diagnostics).
- **Artefacts obligatoires** à chaque run : `manifest.json` (version,
  commit HEAD, rustc), CSV/logs, sortie console, état `state_dir`.
- **Nomenclature** : `run-<banc>-<date>-<commit>` sous `target/`.
- Un banc est « vert » uniquement si **tous** ses oracles sont
  observables dans les artefacts — pas de validation « au ressenti ».
- **Politique d'adresses** : les bancs anti-SSRF doivent démarrer un
  daemon en politique **stricte** — jamais `--offline`, qui sélectionne
  `IpPolicy::permissive()` et rend le rejet SSRF inerte (le refus
  survient alors côté moteur, faux négatif). `sec_anti_ssrf_live.ps1`
  vérifie cette précondition via une sonde témoin `127.0.0.1` : le
  refus attendu est `politique reseau: destination refusee`, pas
  `moteur bittorrent`.

## Format d'entrée

Chaque banc du catalogue §4–§6 partage les champs suivants ; la table
précise les valeurs non triviales.

| Champ | But |
|---|---|
| ID stable | Référence dans commits, CI, tickets |
| Niveau P0–P3 | Ordre de passage objectif (P0 = bloquant release) |
| Script / test | Commande exacte ou fichier `tests/*.rs` |
| Prérequis | Évite les faux échecs (binaire release, ports libres, venv…) |
| Oracle | Succès défini de façon observable |
| Artefacts | Rendent le diagnostic possible |
| Timeout | Détecte blocage et tâche orpheline |
| Réseau | `loopback` / `pyipv8` / `tribler` / `public` |
| Flaky | Causes connues, pour distinguer défaut produit / environnement |
| Dernier run | Date + commit + résultat (§7) — empêche les validations obsolètes |

## 1. Matrice de release — passe P0 (bloquant avant diffusion)

Exécuter dans l'ordre, séquentiellement. Tout échec P0 bloque la
diffusion et tout nouveau développement (ADR-0011 incluse).

| ID | Banc | Script / test | Réseau | Oracle (critère de sortie) |
|----|------|---------------|--------|-----------------------------|
| P0-1 | Régression générale workspace | `scripts/verify_all.ps1` (`cargo check/clippy/fmt/test`, i18n, `flutter analyze/test`, `build web`) | loopback | Tout vert, zéro warning |
| P0-2 | Wire-format PING/PONG | §3.1 + `onionbit-tunnel/tests/` (`MSG_ID` invariant compile-time) | loopback | `TunnelPong::MSG_ID = 7` ; PING ≠ PONG ; pong décodé comme type exact |
| P0-3 | Repos après échange + tempête contrôlée | §3.1 (repos, N pings → N pongs) | loopback | 1 pong/ping ; aucune nouvelle cellule de contrôle après scheduler idle ; queues/CPU bornés |
| P0-4 | Circuits 1–3 sauts + destroy | `circuits_loopback.rs` (READY, écho 2 sauts, destroy) | loopback | Circuits READY, écho exact, relais nettoyés |
| P0-5 | Sortie UDP + politique egress/ingress | `circuits_loopback.rs` (`exit_data`, `tunnel_exit_drops_*`) | loopback | `org_address` réel ; drop non-BT/non-flaggé dans les deux sens |
| P0-6 | SOCKS5 UDP/CONNECT + fake-IP | `circuits_loopback.rs` (associate, fake IP rejetée, lanes isolées, HTTP 28/29) | loopback | Roundtrip exact ; rejet hors RP ; aucune fuite croisée |
| P0-7 | Hidden services e2e + udp_relay | `circuits_loopback.rs` (e2e, retry idempotent, injection forgée, burst) | loopback | Intégrité exacte ; pas de doublon `RP_SEEDER` ; égress relais uniquement |
| P0-8 | Téléchargements moteur | `loopback_download.rs`, `anon_download.rs`, `utp_acceptor.rs`, `dht_backoff.rs` | loopback | Contenu octet à octet ; backoff respecté |
| P0-9 | Kill switch / anti-fuite | `kill_switch_midtransfer.rs`, `circuit_death.rs`, `policy.rs` ×2 | loopback | Zéro datagramme direct pendant la fenêtre morte ; refus fermés |
| P0-10 | Dédoublement `request_one` DHT | §3.2 (fake socket, 1 requête = 1 datagramme) | loopback | Aucun envoi dupliqué |
| P0-11 | Budget entrant DHT | §3.2 (flood → réponses ≤ budget, drops comptés) | loopback | Mémoire/file bornées, compteurs incrémentés |
| P0-12 | Conntrack sortie + exemption IPv8 E2E | §3.3 (source non sollicitée/sollicitée, e2e OK) | loopback | Rejet non sollicité ; hidden-service relay intact |
| P0-13 | Interop Tribler hidden + guards | `interop_hidden_tribler_{download,seed}.ps1 -Guards`, hops 1/2/3 | tribler | SHA-256 exact ; premiers hops multi-hop ⊆ GuardSet |
| P0-14 | Public : bootstrap + download 2 hops + guards | daemon de banc réseau réel (cf. §5 D-94..97) | public | Octets vérifiés > 0 ; guard set non vide ; pinning sticky |
| P0-15 | Mesh fingerprinting — non-régression PING/PONG | `scripts/fingerprint_mesh.ps1 -WithAnonDownload -DurationMin 15` | loopback/mesh | < 5 cellules contrôle/s hors payload ; aucune boucle ping/pong ; comparaison aux CSV de référence `docs/security/fingerprinting.md` |
| P0-16 | Corpus fuzz régression | `scripts/fuzz_campaign.ps1` + `onionbit-tunnel/tests/fuzz_regression.rs` | loopback | 0 crash ; corpus invariant |
| P0-17 | Fuites externes : DNS, kill switch OS, anti-SSRF | `sec_anti_ssrf_live.ps1` (P0-17a) + `sec_leak_capture.ps1` `-Scenario normal` (P0-17b) / `-Scenario kill\|block\|wan\|kill-bootstrap` (P0-17c, cf. §6.1 SE-4) | loopback / public | Refus fermés avec politique stricte vérifiée ; aucun paquet/DNS hors politique ; **INTERDIT=0 dans la fenêtre fail-closed** |

## 2. Niveaux de priorité

- **P0** — bloquant avant diffusion ; vise les garanties touchées par
  les derniers changements (protocole tunnel, DHT vendored, routage de
  sortie, guards, lane anonyme).
- **P1** — produit : API, daemon, persistance/migration, UIs.
- **P2** — capacité : charge, longévité, gros fichiers, essaims denses.
- **P3** — portabilité et scénarios rares : matrice OS, NAT
  inter-NAT, coupure WAN, crash-recovery.

## 3. Bancs ciblés sur les changements récents (P0)

### 3.1 PING/PONG et dispatch des cellules de contrôle

| ID | Banc | Oracle |
|----|------|--------|
| WIRE-1 | Encoder `TunnelPing`/`TunnelPong`, vérifier `PING != PONG`, `TunnelPong::MSG_ID = 7`, décoder chaque paquet → type exact | Invariant compile-time (déjà posé par `920501e`) + test de décodage |
| WIRE-2 | Repos après échange : A ping B → B répond une fois → A ne répond pas au pong → après plusieurs tours de scheduler, zéro cellule de contrôle émise | Compteur de cellules stable ; pas de trafic auto-entretenu |
| WIRE-3 | Tempête contrôlée : injecter N pings concurrents → exactement N pongs, pas de croissance récursive, queues/CPU/cellules sortantes bornées | `pongs == N` ; bornes respectées |

### 3.2 DHT vendored et socket de lane anonyme

| ID | Banc | Oracle |
|----|------|--------|
| DHT-D1 | Dédoublement `request_one` : fake socket enregistrant les envois → 1 requête logique = 1 datagramme | `sent == 1` par requête |
| DHT-D2 | Budget entrant : flood synthétique de requêtes DHT valides → réponses envoyées ≤ budget, drops comptés, mémoire/file bornées | Compteurs de drop ; mémoire stable |
| DHT-D3 | Stall longue : magnet sans metadata ni peers pendant 30 min → p50/p95 datagrammes/s et cells/s, backoff+jitter+plafond prouvés, aucun trafic direct, kill switch intact | Débit décroissant plafonné ; 0 datagramme direct |
| DHT-D4 | Fonctionnel après limitation : torrent réel seedé, lane anonyme 2 hops → découverte OK, contenu vérifié, délai de découverte mesuré avant/après | Contenu exact ; délai chiffré (compromis documenté) |

### 3.3 Conntrack de sortie et exemption IPv8 E2E

| ID | Banc | Oracle |
|----|------|--------|
| CT-1 | Source non sollicitée : datagramme UDP non-IPv8 entrant au socket exit sans destination récemment contactée → rejet, aucun retour, aucune allocation non bornée | Rejet observable ; tables bornées |
| CT-2 | Source sollicitée : émission préalable vers adresse externe simulée → réponse de cette adresse acceptée et routée au circuit | Réponse livrée au bon circuit |
| CT-3 | Exception IPv8 E2E : `created-e2e`/`linked-e2e` et relais hidden-service fonctionnels, non cassés par conntrack | E2e hidden service vert sous conntrack actif |

## 4. Catalogue — bancs loopback automatisés (A)

Réseau : `loopback`. Artefacts : sortie `cargo test` + traces. Niveau
par défaut P1 sauf mention contraire (les lignes reprises en P0
renvoient à §1/§3).

### 4.1 IPv8 / découverte (`onionbit-ipv8`)

| ID | Banc | Oracle |
|----|------|--------|
| IP-1 | Ping/pong signé 2 nœuds | Signatures valides, pairs mutuellement enregistrés |
| IP-2 | `introduction-request/response` ancien + nouveau style (233/234) | Flag `intro_supports_new_style` propagé |
| IP-3 | `puncture-request` (non signé) → `puncture` (signé) | Chemins `wan_walker`/`lan_walker` respectés |
| IP-4 | Marche aléatoire ≥3 nœuds | Adresses walkable propagées |
| IP-5 | Horloge de Lamport / global time `% 65536` | Horloges cohérentes |
| IP-6 | Blacklist adresse + MIDs | `add_verified` refusé, pair rejeté |
| IP-7 | `my_estimated_wan/lan` via `destination_address` | Estimation correcte par sous-réseau |

### 4.2 DHT overlay

| ID | Banc | Oracle |
|----|------|--------|
| DH-1 | `store_value`/`find_values` signés bout en bout | Valeur retrouvée, signature vérifiée |
| DH-2 | Jetons `sha1(node+secret)` : accept/reject, rotation | Token évincé rejeté, frais accepté |
| DH-3 | Crawl itératif, `find_nodes`, distance XOR, buckets 8 + split | Table conforme |
| DH-4 | Messages 1–10, présence/absence `GlobalTimeDistributionPayload` par msg_id | Filaire conforme pyipv8 |
| DH-5 | Maintenance `node`/`value`/`token` aux cadences nominales | Cadences 0,5 s/60 s/3600 s/300 s |
| DH-6 | `connect_peer` (noblockdht) | Fire-and-forget, pair joint |

### 4.3 Tunnels / circuits (`onionbit-tunnel`)

| ID | Banc | Oracle |
|----|------|--------|
| TU-1 | Circuits 1/2/3 sauts → READY (`create/created`, `extend/extended`) | État READY, route exposée par `circuit_info` |
| TU-2 | Sortie `exit_data` dernier saut + retour | `org_address` = source réelle |
| TU-3 | Écho uTP 2 sauts | Payload identique |
| TU-4 | `destroy` signé | Circuit + relais nettoyés |
| TU-5 | Ping/pong circuit, `relay_early` borné 8 | Cf. §3.1 (P0) |
| TU-6 | Flags de sortie via intros tunnel (`extra_bytes` bitmask) | `flag_registry`, `get_candidates`, marquage tardif |
| TU-7 | Candidats `RELAY`-only refusés ; `required_exit` exclu du `pick_first_hop` | Pas d'auto-élection |
| TU-8 | `build_circuits` ignore les IP seeders | Pool candidats correct |
| TU-9 | Tempête destroy → reconstruction bornée | Pas de tempête de create ; guards bornent premiers hops |
| TU-10 | Speedtest : cellules 21/22 u32 + 19/20 u16, flux `speed:` | Lignes `{"up","down"}` MiB/s |
| TU-11 | `estimate_swarm_size` (peers-request, DHT puis PEX), quirk `?hops=` | Comptage `seeder_pk` uniques PEX |
| TU-12 | `PexStore` (intro/stop_announce, TTL 300 s, borne 20) | TTL et borne respectés |
| TU-13 | Rupture saut intermédiaire en trafic | Détection, nettoyage, reconstruction |
| TU-14 | `tunnel_udp_socket` roundtrip + pinning circuit par destination | Pinning stable |

### 4.4 SOCKS5 / HTTP tunnel

| ID | Banc | Oracle |
|----|------|--------|
| SO-1 | Greeting sans auth, `UDP ASSOCIATE` roundtrip via circuit `DATA` | Payload identique |
| SO-2 | Sélection sticky destination → circuit `READY` du bon `goal_hops` | Circuit correct réutilisé |
| SO-3 | IPv4 factice `circuit_id_to_ip` rejetée hors circuit RP | Rejet sans repli `DATA` |
| SO-4 | Deux lanes anonymes isolées | Aucune fuite croisée |
| SO-5 | `CONNECT` HTTP → cellules 28/29, chunks 1400 o, `Content-Length`/`chunked`, borne 5 requêtes, `EXIT_HTTP` | Réponse reconstituée |
| SO-6 | Politique de sortie dans les deux sens | Drop non-BT/non-flaggé |

### 4.5 Hidden services e2e

| ID | Banc | Oracle |
|----|------|--------|
| HS-1 | Cycle complet intro → peers → RP downloader/seeder → `create/link_e2e` → données | Intégrité exacte |
| HS-2 | Retry `create_e2e` idempotent | ID+DH stables, dédup, `link-e2e` retransmis ré-repondu, 0 doublon `RP_SEEDER` |
| HS-3 | Égress uniquement vers relais | Aucune fuite directe |
| HS-4 | Injection paquets forgés | Rejet propre |
| HS-5 | Annonce DHT points d'intro + lookup e2e | Lookup résout, données OK |
| HS-6 | `udp_relay` `dial`/`serve`, `first-seen`, `dial_to` filaire | Roundtrip exact, lock respecté |
| HS-7 | Intégrité sous rafale RP_SEEDER | Contenu exact sous burst |

### 4.6 Moteur BitTorrent (`onionbit-bittorrent`)

| ID | Banc | Oracle |
|----|------|--------|
| BT-1 | Download loopback seeder+leecher rqbit | Octet à octet |
| BT-2 | Download anonyme via hidden service (uTP sur circuit e2e) | Octet à octet |
| BT-3 | Magnet sans métadonnées (`ut_metadata`) | Info récupérée |
| BT-4 | Backoff DHT moteur | Pas de tempête de queries |
| BT-5 | Accepteur uTP entrant | Connexion acceptée |
| BT-6 | `selected_files`, `recheck`, `move_storage` + re-hash | Sélection/déplacement corrects |
| BT-7 | Limites débit par torrent + globales | `ratelimits` appliqués |
| BT-8 | `seeding_ratio`/`seeding_mode`/`safe_seeding` | Arrêt de seed conforme |
| BT-9 | File : `queue_position`, `auto_managed`, `active_*` | Ordre respecté |
| BT-10 | Pause/reprise/suppression ± `remove-data` | États et fichiers cohérents |

### 4.7 Core / services (`onionbit-core`, `onionbit-db`)

| ID | Banc | Oracle |
|----|------|--------|
| CO-1 | Persistance downloads + `restore_downloads` après redémarrage | État restauré |
| CO-2 | Migrations v1→v2→v3 | Données conservées, idempotent, `SchemaTooNew` refusé |
| CO-3 | Watch folder `.torrent`/`.magnet` dédupliqué | Import unique |
| CO-4 | Torrent checker scrape UDP BEP-15 + HTTP | `torrent_state` + notif ; **jamais** d'infohash anonyme |
| CO-5 | RSS : ETag, découverte+notif, anti-SSRF, `rss_items` | Flux conditionnel, URL niée refusée |
| CO-6 | `ContentDiscovery` (msgs 3/4, 101/102, 201/202) | Remote-select fonctionnel |
| CO-7 | Guards : adoption, bornage premiers hops sous storm, exclusion de soi, persistance/redémarrage, set désactivé non chargé | `guards.rs` vert |
| CO-8 | File multi-lanes | Ordonnancement correct |

### 4.8 Kill switch / politiques (`onionbit-network-policy`)

| ID | Banc | Oracle |
|----|------|--------|
| KS-1 | Proxy mort en plein transfert (portée `proxy`) | `add`/`resume` bloqués, reprise à guérison |
| KS-2 | Circuit détruit, proxy vivant (portée `circuits`) | 0 datagramme destination pendant fenêtre morte |
| KS-3 | Proxy injoignable au démarrage | Refus, jamais de repli direct |
| KS-4 | URI http(s) vers IP niée | Refus strict / acceptation permissif |
| KS-5 | `socks5_proxy` non-loopback | Moteur ne démarre pas |

### 4.9 API REST/SSE (`onionbit-api`) — `tests/api.rs`

| ID | Banc | Oracle |
|----|------|--------|
| AP-1 | Downloads CRUD + PATCH complet + sous-endpoints | Codes/DTO conformes Python |
| AP-2 | SSE `/api/events` : tous les topics, `events_start.public_key` | Format `event:/data:`, topics émis |
| AP-3 | Auth `X-Api-Key`/`?key=`/cookie | 401 `{handled:true}`, aucun `/api` exempté |
| AP-4 | `settings` GET/POST : merge, persistance, restart-only | `configuration.json` écrit |
| AP-5 | `ipv8/*` : overlays, network, isolation, noblockdht, statistics (400/412) | Sémantique Python |
| AP-6 | `ipv8/tunnel/*` : circuits/relays/exits/swarms/peers dht+pex/guards/speedtest | Collections vides en 200 si `tunnels is None` |
| AP-7 | `asyncio/*` : drift (hist. 100), tasks, debug GET/PUT | Shapes conformes |
| AP-8 | `metadata`, `search/remote`, `torrentinfo`, `createtorrent`, `libtorrent/*`, `files`, `rss`, `versioning`, `logging`, `statistics`, `shutdown` | Contrats `api_rest_mapping.md` |
| AP-9 | Limites : body 16 Mio, corps malformés, codes 400/404/500/412 | Parité erreurs Python |

### 4.10 Daemon / CLI / statiques web

| ID | Banc | Oracle |
|----|------|--------|
| DA-1 | `--offline` : API joignable, refus non-loopback, shutdown Ctrl-C + `PUT /api/shutdown` | Arrêt propre du processus |
| DA-2 | CLI réel : `status/list/add/remove/pause/resume`, cas injoignable, `--api-key` | Sorties conformes, erreurs stderr |
| DA-3 | Statiques web : index, assets, fallback SPA, exemption auth limitée, anti-traversée, `web_ui_inject_key`, en-têtes | 401 `/api` sans clé, statiques 200 |
| DA-4 | Mutex instance unique par `state_dir` | Second lancement silencieux |
| DA-5 | Listener HTTPS : cert PEM/auto-signé persisté, `https_port_running`, grace 5 s | TLS joignable, arrêt propre |

### 4.11 Messagerie e2e (ADR-0011, Phase 8)

Ces bancs deviennent exécutables à mesure que les étapes 36-40
livrent ; **P0 pour la messagerie** = bloque l'activation par
défaut de la fonction (pas la release existante). Harness :
`make_node`/`wait_two` de `circuits_loopback.rs` + démon de banc.

| ID | Banc | Oracle | Niveau |
|----|------|--------|--------|
| MS-1 | Cycle complet : `join_swarm(messaging_hash(pk))` → annonce DHT → `peers-request` → RP → `link-e2e` → `hello` → consentement → message → `ack` | Corps exact, `delivered=true`, SSE `message_received`+`message_delivered` | P1 |
| MS-2 | Réouverture de circuit : mort du circuit e2e puis renvoi | Re-liaison transparente ; trames réémises absorbées par dédup (0 doublon en historique) | P1 |
| MS-3 | Authentification : trame signée par une autre clé, `sig` tronquée/absente, corps modifié post-signature | Rejet systématique, 0 écriture `messages` | P0-msg |
| MS-4 | Codec hostile : `v` inconnu, champ critique absent/inconnu, bencode malformé, trame > 32 Kio, `body` > 30 Kio ; cible fuzz `tunnel_payloads` | Rejet borné, 0 panic, parse borné | P0-msg |
| MS-5 | Anti-replay/ordre : `seq` rejoué, `seq` hors fenêtre 64, `id` dupliqué, désordre dans la fenêtre | Rejoués droppés sans réponse ; désordre en fenêtre livré une fois | P0-msg |
| MS-6 | Consentement : non-`hello` d'inconnu droppé ; `pending` sous rafale (capacité/TTL 10 min) ; refus → `blocked` → trames suivantes rejetées + liens e2e du swarm refusés | État borné, aucun historique d'inconnu | P0-msg |
| MS-7 | Offline : contact hors ligne → `Undeliverable` à timeout borné | État `failed` visible API/UI ; 0 file, 0 réémission, 0 boucle | P1 |
| MS-8 | Fingerprint : `fingerprint_mesh.ps1` messagerie active vs inactive (annonces de présence périodiques) | Delta mesuré et consigné dans `fingerprinting.md` ; aucune boucle de contrôle | P0-msg |
| MS-9 | Non-fuite : capture loopback — trames messagerie uniquement sur circuits e2e ; la lane uTP BitTorrent ne voit jamais de trame `v=1` ; 0 datagramme direct | 0 octet hors tunnel ; compteurs lane BT inchangés | P0-msg |
| MS-10 | DoS applicatif : flood par contact puis global → trames traitées ≤ budgets ; drops comptés ; mémoire/état bornés | Compteurs de drop ; bornes respectées | P1 |
| MS-11 | Persistance : restart — contacts/messages/`seq` restaurés ; `DELETE` physique ; `retention_secs` purge | Historique et compteurs repris ; rien de ré-emis | P1 |
| MS-12 | API : `GET/POST /api/messages`, `GET/POST/DELETE /api/contacts` ; 401 sans clé ; corps malformés | Contrats conformes `api_rest_mapping.md` | P1 |

## 5. Catalogue — bancs interop et réseau réel

### 5.1 Interop pyipv8 (venv `TRIBLER_SRC`) — réseau `pyipv8`

| ID | Script | Oracle | Niveau |
|----|--------|--------|--------|
| PY-1 | `interop_ipv8.ps1` | Paquets signés acceptés deux sens par `default_eccrypto`, pairs enregistrés | P0 |
| PY-2 | `interop_discovery.ps1` | 234→233, 232→231, 250→249 deux sens, signatures via `lazy_wrapper` | P0 |
| PY-3 | `interop_dht.ps1` | `find_values`/`store_value` signés bidirectionnels, rotation secrets | P0 |
| PY-4 | `interop_tunnel.ps1` | `create`/`created`, crypto ChaCha20-Poly1305 acceptée par `decrypt_str`, clés de session identiques | P0 |
| PY-5 | `interop_py_relay.ps1` | Circuit Rust → pyipv8 relais → Rust sortie | P1 |
| PY-6 | `interop_exit_download.ps1` | Download rqbit réel via sortie pyipv8 `EXIT_BT`, octet à octet | P0 |
| PY-7 | `interop_hidden_py2py.ps1` | Baseline protocole py↔py | P1 |
| PY-8 | `tests/interop_replay.rs` + fixtures | Fixtures pyipv8 rejouées en CI | P0 |

### 5.2 Tribler.exe installé — réseau `tribler`

| ID | Script | Oracle | Niveau |
|----|--------|--------|--------|
| TR-1 | `interop_tribler.ps1` | Intro + flags réels `{RELAY, SPEED_TEST}`, circuit 2 sauts Rust→Tribler→Rust, écho uTP | P0 |
| TR-2 | `interop_tribler_relay.ps1` / route Tribler 1er saut | 3 sauts, 4 Mio, route épinglée | P1 |
| TR-3 | `interop_hidden_tribler_download.ps1` | OnionBit seede → Tribler télécharge | P0 |
| TR-4 | `interop_hidden_tribler_seed.ps1` | Tribler seede → OnionBit télécharge | P0 |
| TR-5 | Matrice `-Guards` hops 1/2/3 × sens A/B + baseline `-Guards` off | SHA-256 exact, premiers hops ⊆ GuardSet / premier hop libre | P0 |
| TR-6 | `interop_hidden_killseeder.ps1` | Mort du seeder en transfert : pas de fuite, erreur propre | P1 |
| TR-7 | `api_parity.ps1` | Même requêtes `Tribler.exe -s` vs daemon Rust, diff documenté (ADR) | P1 |

### 5.3 Réseau public — réseau `public`

| ID | Banc | Oracle | Niveau |
|----|------|--------|--------|
| PU-1 | Bootstrap réel + marche aléatoire | Pairs publics, `my_estimated_wan` derrière NAT | P0 |
| PU-2 | Download anonyme public 2 sauts (magnet public) | Octets > 0, exits/guards réels, kill switch engagé→READY, 0 fallback | P0 |
| PU-3 | Guards réels : adoption, pinning sticky, `failures`, persistance + reload | Set actif+réserve, reload identique | P0 |
| PU-4 | DHT public (`interop_public_dht.ps1`) | store/find/announce sur DHT Tribler réel | P1 |
| PU-5 | Hidden seeding live (`live_hidden_upload.ps1`, `sync_public.ps1`) | Leechers publics servis anonymement | P1 |
| PU-6 | Puncture NAT inter-NAT réel (2 NAT distincts) | Trou réel percé — **jamais exécuté** | P3 |
| PU-7 | Longévité ≥ plusieurs heures | Stabilité circuits, remplacement, pas de fuite mémoire/tâche | P2 |

## 6. Catalogue — sécurité, produit, charge

### 6.1 Sécurité dédiée — réseau variable

| ID | Banc / script | Oracle | Niveau |
|----|---------------|--------|--------|
| SE-1 | `fingerprint_mesh.ps1`/`fingerprint_stats.ps1` : idle 20 min, e2e actif, mesh contrôle, lane anonyme | Cadences ≤ références `fingerprinting.md` ; cf. P0-15 | P0 |
| SE-2 | `fuzz_campaign.ps1` + `fuzz_regression.rs` | 0 crash, corpus invariant, `fuzz_journal` mis à jour | P0 |
| SE-3 | `sec_leak_capture.ps1` + `analyze_leak_capture.py` : capture pktmon complète pendant download anonyme réel, attribution par port local du banc (TAP = vérité fil) | 0 paquet INTERDIT depuis les ports du banc ; qnames DNS rapportés et revus | P0 |
| SE-4 | Kill switch OS réel (P0-17c) : `sec_leak_capture.ps1 -Scenario {kill,block,wan,kill-bootstrap}` — kill du banc en plein transfert, pare-feu sur premiers sauts réels (mort de circuit, proxy vivant — un fallback direct resterait VISIBLE), Disable/Enable-NetAdapter, mort du bootstrap Tribler | INTERDIT=0 entre `t_failure` et `t_fin_capture` ; manifeste : `t_failure`, dernier octets, reprise observée | P0 |
| SE-5 | `sec_anti_ssrf_live.ps1` : auth 401 + anti-SSRF live (link-local, `::1`, `localhost`, privé, `0.0.0.0`) sur daemon strict | Précondition politique stricte vérifiée par sonde témoin ; refus fermé avec raison exacte | P0 |
| SE-6 | Auth API : chaque endpoint sans clé, fixation cookie | 401 systématique | P1 |
| SE-7 | Messagerie e2e (Phase 8) : replay inter-circuits, usurpation de clé, fingerprint des annonces de présence — renvoie aux oracles MS-3/MS-5/MS-8/MS-9 | Les quatre oracles verts | P0-msg |

### 6.2 Produit : packaging, systray, UIs — réseau `loopback`

| ID | Banc | Oracle | Niveau |
|----|------|--------|--------|
| PR-1 | `build_release.ps1` + smoke daemon | Binaire + manifest ; API + shutdown | P1 |
| PR-2 | Systray : menu, « Démarrer avec Windows » (Run HKCU), logs, Quitter, pas de double icône | Registre = source de vérité | P1 |
| PR-3 | Lanceurs `demarrer.ps1`/`arreter.cmd`/`OnionBit Web.cmd`, `--console`/`--no-tray` | Parcours complet | P1 |
| PR-4 | Flutter desktop live : endpoints 200, SSE `events_start`, validation visuelle | Rendu vérifié à la souris | P1 |
| PR-5 | Flutter web multi-navigateurs : drop HTML5, player `/stream`, notifications, PWA, responsive, saisie clé | **Passe manuelle restante** | P1 |
| PR-6 | i18n EN/FR à chaud, pluriels ICU (`check_i18n.ps1`) | 0 littéral manquant | P1 |
| PR-7 | `versioning/check` sonde réelle + `tribler_new_version` | Notification émise | P2 |
| PR-8 | Matrice OS : ARM64-Windows, Linux, macOS | **Jamais exécuté** — exige CI matricielle | P3 |

### 6.3 Charge / limites — réseau `loopback`/`public`

| ID | Banc | Oracle | Niveau |
|----|------|--------|--------|
| CH-1 | 50+ torrents simultanés, `active_*` saturés | Dégradation propre | P2 |
| CH-2 | Fichier multi-Gio + disque presque plein | `low_space`, comportement `saveas` | P2 |
| CH-3 | Essaim dense, `max_peers`, `reverse_intro` FIFO 500 | Bornes respectées | P2 |
| CH-4 | Rafales UDP entrantes | Drop tail borné, 0 crash | P1 |
| CH-5 | Coupure WAN complète puis retour | Reprise sans état corrompu | P2 |
| CH-6 | `bench_crash_recovery.ps1` : N cycles (démarrage `--offline` → PUT `.torrent` → taskkill -F à instant pseudo-aléatoire → redémarrage même state-dir) | API remonte à chaque crash ; tous les downloads persistés restitués (`infohash` par infohash) ; `PRAGMA quick_check` = `ok` | P2 |
| CH-7 | Dérive tâches périodiques longue durée | `asyncio/drift` borné | P3 |

## 7. Journal des runs

| ID banc | Dernier run | Commit | Résultat | Artefacts | Flaky |
|---------|-------------|--------|----------|-----------|-------|
| P0-1 (`verify_all` complet : check/clippy/fmt/tests + i18n + Flutter analyze/test/build web) | 2026-10-02 | `920501e` (+ docs non commités) | **vert** — 0 warning, tous tests Rust + 19 tests Flutter | sortie console | non |
| P0-2/P0-3 (tunnel : WIRE, PING/PONG, circuits, sortie, SOCKS5, HS, conntrack) | 2026-10-02 | `920501e` | **vert** — 38 tests `circuits_loopback` + 6 `fuzz_regression` + unitaires | sortie `cargo test -p onionbit-tunnel` | non |
| P0-8 (moteur : loopback, anon e2e, uTP, DHT backoff/budget) | 2026-10-02 | `920501e` | **vert** — `loopback_download`, `anon_download` (2,8 s), `dht_backoff` ×4, `utp_acceptor` ×2 | sortie `cargo test -p onionbit-bittorrent` | non |
| P0-9 (kill switch : proxy mort mid-transfer, circuit_death, policy) | 2026-10-02 | `920501e` | **vert** — `kill_switch_midtransfer` (25 s, 0 fuite), `circuit_death`, `policy` ×2 | idem + `cargo test -p onionbit-core` | non |
| P0-10/P0-11 (DHT : budget entrant, cadence stall plafonnée) | 2026-10-02 | `920501e` | **vert** — `budget_requetes_entrantes_borne_les_reponses`, `magnet_stall_cadence_decroit_vers_le_plafond` | `dht_backoff.rs` | version courte (30 min → ~9 s accéléré) |
| CO-*/migrations/guards/queue | 2026-10-02 | `920501e` | **vert** — lifecycle, guards, queue, services, network-policy ×13, db ×10 | `cargo test -p onionbit-core -p onionbit-network-policy -p onionbit-db` | non |
| AP-*/DA-*/IP-*/DH-* (api 59, daemon 2, cli 2, ipv8, format, crypto) | 2026-10-02 | `920501e` | **vert** | `cargo test -p onionbit-api …` | non |
| PY-1 (`interop_ipv8.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — paquets Rust vérifiés par pyipv8 `38/38`, Python `36/36`, pairs mutuels | `target/interop` | non |
| PY-2 (`interop_discovery.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — intro/puncture anciens+nouveaux formats, `INTEROP DISCOVERY OK` | `target/interop` | non |
| PY-3 (`interop_dht.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — store/find signés bidirectionnels, jeton périmé rejeté | `target/interop` | non |
| PY-4 (`interop_tunnel.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — crypto par couches acceptée par le vrai `TunnelCommunity` | `target/interop` | non |
| PY-5 (`interop_py_relay.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — 3 sauts Rust→relais pyipv8→sortie Rust (script corrigé : `rust_stderr.log` tolérant) | `target/interop-py-relay*` | non |
| PY-6 (`interop_exit_download.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — 200 Ko via circuit 2 sauts, sortie pyipv8 `EXIT_BT`, octet à octet | `target/interop` | non |
| PY-7 (`interop_hidden_py2py.ps1`) | 2026-10-02 | `920501e`+docs | **BLOCKED / ENVIRONMENTAL** — baseline Tribler↔Tribler échouée : `swarm_ips_dht=0` pendant 38 min, aucun OnionBit dans le chemin → aucun jugement sur l'interop hidden OnionBit. À réessayer lors d'une fenêtre où la baseline officielle est verte | `target/interop-hidden-py2py*` | réseau public |
| TR-1 (`interop_tribler.ps1`) | 2026-10-02 | `920501e`+docs | **vert** — flags réels `9`, circuit 2 sauts via Tribler, écho uTP exact | `target/interop-tribler*` | non |
| TR-3 (`interop_hidden_tribler_download.ps1 -Guards`) | 2026-10-02 | `920501e`+docs | **vert** hops 1/2/3 — SHA-256 exact, 6,3 Mo, `hors_set=0`, premiers hops ⊆ guard set | `target/interop-hidden-dl-*` | 1er run hops=2 pollué par un daemon orphelin (port 8097/17787), vert après nettoyage |
| TR-4 (`interop_hidden_tribler_seed.ps1 -Guards`) | 2026-10-03 | `fa24b6b`+scripts corrigés | **vert** hops 1/2/3 — tous verdicts OK ; course fichier→handle corrigée (DELETE download `remove_data=false` avant hash + retry 30 s) ; **re-run hops=1 post-fix : EXIT=0, SHA-256 in-script OK** | `target/interop-hidden-seed-20261003-013948`, `target/seed-h1-rerun.log` | non |
| P0-15 (`fingerprint_mesh.ps1 -WithAnonDownload 15 min`) | 2026-10-03 | `fa24b6b` | **vert** — pas de tempête PING/PONG : ping+pong Discovery ≈ 0,36 msg/s ; `on_cell` tunnel ≈ 1,28 msg/s ; kill switch engagé→désarmé ; 0 fallback | `target/fingerprint-mesh-20261003-004639/` (2 CSV 180 éch., manifest) | non ; le magnet factice ne transfère pas (voulu) |
| P0-14 (`interop_public_dht.ps1` 2 sauts, Sintel) | 2026-10-03 | `fa24b6b` | **vert** — `INTEROP PUBLIC DHT OK`, **1 638 263 octets vérifiés** ≥ 1 Mio, 1er essai, route 2 sauts publics réels `605f9289…→0f6a1aee…` | `target/interop-public-dht/*.log` | overlay clairsemé ce soir (3 pairs, 1 exit) ; guards non observables par ce harness (prouvés par TR-3/TR-4) |
| P0-16 (`fuzz_campaign.ps1 -Smoke`) | 2026-10-03 | `fa24b6b` | **vert** — 6/6 cibles, 0 crash, ~152 M execs (tunnel_cell 36,4 M ; tunnel_payloads 9,4 M) | `fuzz/artifacts/last-run-*.log`, `docs/security/fuzz_journal.csv` | non ; smoke ≠ campagne 5 h |
| P0-17a (`sec_anti_ssrf_live.ps1` : auth + anti-SSRF live) | 2026-10-03 | `fa24b6b`+script | **vert** — EXIT=0, 11/11 verdicts : précondition stricte vérifiée par sonde témoin, 401 sans/mauvaise clé, 200 avec clé, refus fermés avec raison exacte (link-local, loopback, privé/CGNAT, unspecified) ; précondition prouvée discriminante sur daemon `--offline` | `target/ssrf-run.log`, `target/sec-ssrf-*/` | non |
| P0-17b (`sec_leak_capture.ps1` + `analyze_leak_capture.py`) | 2026-10-03 | `5f23a3b`+scripts | **vert** — download anonyme 2 sauts réel (1 113 975 o vérifiés, route `5a6f405d…→d84eb3cd…`) sous capture pktmon complète : **INTERDIT = 0** sur les ports du banc (attribution par port local, 181 770 paquets), 31 202 paquets overlay, fenêtre post-arrêt 20 s propre ; DNS : uniquement infra DHT (`dht.*`, `router.*`) + bruit OS — aucun hostname tracker/magnet. Reste : rejouer sous destruction circuit / proxy tué / coupure WAN (SE-4) | `target/leak-capture-20261003-093224/` (pcapng, manifest, rapport JSON) | non ; attribution port local (pktmon ne porte pas de PID) |
| P0-17c — fail-closed OS, 4 sous-runs | 2026-10-03 | `a989b96`+scripts | **vert ×4** — `kill` : taskkill à 262 144 o vérifiés, 0 interdit fenêtre post-mortem 25 s (`target/leak-capture-20261003-095632/`) ; `block` : pare-feu premiers sauts `[127.0.0.1, 192.42.116.243]` à 327 543 o, 0 interdit, **reprise** jusqu'à 982 903 o (`…-095810/`) ; `wan` : `Ethernet` coupé 45 s à 589 687 o, 0 interdit, reprise → 4 915 063 o (`…-100702/`) ; `kill-bootstrap` : Tribler.exe tué à 851 831 o, download **complété** à 1 638 263 o, 0 interdit (`…-101015/`). Correctif analyseur en route : signature IPv8 (`00 02`+cid) = overlay structurel admis — le TAP ne couvre que les lanes, pas les paires candidates (run `…-093921/` : 2 050 faux positifs reclassés, DHT-tunnel en échec environnemental ce run). 17c-5 (réinstanciation de lane) : non injectable sans hook daemon — reste en trou §8 | pcapng+manifeste par run | non ; proxy/worker in-process (17c-2 ↦ kill-bootstrap), download parfois stalle côté DHT-tunnel |
| PR-1 (`build_release.ps1` + smoke release) | 2026-10-03 | `03a5307` | **vert** — release `x86_64-pc-windows-msvc` en 5 min 15 s, manifeste complet (commit, rustc 1.98.1, version 0.6.0-alpha) ; smoke du binaire `dist/x86_64-pc-windows-msvc/onionbit-daemon.exe` : API montée, 401 sans clé, arrêt propre | `dist/x86_64-pc-windows-msvc/` | non |
| PR-3 (lanceur `OnionBit Web.cmd` → `web-launch.ps1`) | 2026-10-03 | dist alpha + `03a5307` | **vert partiel** — le lanceur démarre `onionbit-daemon.exe --state-dir dist\state`, API joignable, navigateur ouvert ; parcours UI complet reste manuel | `dist/OnionBit-0.6.0-alpha-windows-x64/state/` | daemon dist = ancien build |
| CH-6 (`bench_crash_recovery.ps1`, 5 cycles) | 2026-10-03 | `03a5307`+script | **vert** — 5 × (démarrage `--offline` → PUT `.torrent` → taskkill -F à instant aléatoire) : API remontée à chaque crash, 2 downloads persistés restitués à chaque redémarrage, `PRAGMA quick_check` = `ok`. Leçon : un magnet non résolu n'est **pas** persisté (par design — `resolve_magnet` bloque) ; le chemin persisté est `torrent_data` | `target/crash-recovery-*/` | non |
| CH-1 (50 torrents `.torrent` distincts, `--offline`) | 2026-10-03 | `fdc41f5` | **vert** — 50/50 PUT en 0,45 s, 50 listés, RSS 27→31 MB stable, aucune erreur ; variante forte (swarm actif) reste un banc public | `target/load-state/`, `target/load-torrents/` | non |
| Longévité bornée (`fingerprint_mesh.ps1 -DurationMin 60 -WithAnonDownload`) | 2026-10-03 | `402753e` | **vert** — 718 échantillons ×2 sur 60 min : ping/pong 0,366→0,362 msg/s (dérive nulle, aucune tempête), cellules tunnel 1,79/s, circuits 1→3 reconstruits proactivement, daemon vivant en fin de run ; le WARN `failed to forward port` récurrent est l'UPnP de la machine (Tribler.exe), bruit bénin | `target/fingerprint-mesh-20261003-103013/` | non |
| CH-4 (rafales UDP entrantes) | 2026-10-03 | tests | **couvert** — `inject_incoming_burst_drop_tail` + `inject_incoming_soak` (tunnel) + `budget_requetes_entrantes_borne_les_reponses` (DHT) verts dans la passe P0 ; pas de banc OS dédié supplémentaire | tests tunnel/bittorrent | non |
| PR-2 (systray / autostart) | 2026-10-03 | `fdc41f5` | **vérifié statiquement** — `autostart.rs` écrit/lit `HKCU\…\Run\TriblerRustDaemon`, `is_enabled()` pilote la case tray ; le toggle est un clic menu → parcours complet manuel | `crates/onionbit-daemon/src/autostart.rs` | manuel |
| PR-7 (`versioning/versions/check`, daemon `--no-ipv8`) | 2026-10-03 | `402753e` | **vert partiel** — sonde exécutée : `{has_version:false, new_version:""}`, `current=0.6.0-alpha` ; la notification `tribler_new_version` n'est observable que si une release distante plus récente existe | `target/ver-*.log` | non |

## 8. Trous de couverture assumés / hors scope

- **Matrice OS** (PR-8) : toolchains absentes localement → CI requise.
- **Sortie Tribler.exe** : `exitnode_enabled` non exposable → couvert
  par PY-6 (sortie pyipv8).
- **`identity/*`** : exclu par ADR-0006.
- **Exposition LAN / `cors_origins` / pilote mobile distant** : Phase
  7b, hors V1.
- **FFI mobile embarqué** : abandonné (étape 19 remplacée), surface
  figée en référence.
- **P0-17c-5** (réinstanciation d'une lane anonyme sous capture OS) :
  non injectable depuis le banc `interop_public_download` — requiert un
  hook daemon/API de reset de lane ; les 4 autres pannes (circuit,
  worker/bootstrap, WAN, processus) sont couvertes.
- **P0-17c-2 au sens strict** (tuer le worker SOCKS seul) : le proxy
  tunnel est in-process dans le banc — pas de PID séparable ;
  approximation validée = mort du bootstrap Tribler + mort du
  processus complet (`kill`).
