# Mesure du fingerprinting d'implementation

Le threat model liste comme residu le **fingerprinting de
l'implementation** : des differences de timing, tailles ou retries
entre OnionBit (Rust) et Tribler (pyipv8) peuvent rendre un noeud
identifiable sur le reseau. Ce document decrit la procedure de
mesure — un livrable analytique, pas un gate de securite.

## Principe

Pas de PCAP brut : on collecte des **statistiques agregees** via les
compteurs REST, identiques en shape entre OnionBit et Tribler
(`GET /api/ipv8/overlays/statistics` — `num_up`/`num_down`/
`bytes_up`/`bytes_down` par overlay et `msg_id` ; `GET
/api/ipv8/tunnel/circuits` — circuits par etat). Les deltas entre
echantillons donnent debits par message, cadences keepalive et
rafales.

## Procedure

```powershell
# OnionBit : 20 min idle + session de transfert
.\scripts\fingerprint_stats.ps1 -ApiBase http://127.0.0.1:52100 `
    -ApiKey <cle> -DurationMin 20 -IntervalSec 5 `
    -OutCsv fingerprint_onionbit_idle.csv

# Tribler officiel : meme commande sur son API (port configure)
.\scripts\fingerprint_stats.ps1 -ApiBase http://127.0.0.1:<port> `
    -ApiKey <cle_tribler> -DurationMin 20 `
    -OutCsv fingerprint_tribler_idle.csv
```

Comparer ensuite, par overlay/msg_id :

- **cadence** : messages/seconde en idle (walks, keepalives, pings
  de circuit) — un ecart de cadence constant est un signal fort ;
- **volumes** : bytes/msg_id moyens — une divergence de taille des
  enveloppes ou du padding est un signal ;
- **rafales** : pics de `num_up` sur un echantillon — cadence de
  maintenance differemment bruitee ;
- **circuits** : cycles de vie (READY/EXTENDING) dans le temps —
  politiques de reconstruction differentes.

## Limites

- Les compteurs sont agrégés par msg_id — le timing intra-message
  (inter-arrivee de datagrammes) n'est pas visible a ce niveau ;
  un tap de datagrammes (`UdpEndpoint::set_tap`) pourra affiner si
  un signal grossier apparait.
- Idle de testbench != idle de reseau reel (population de pairs,
  churn). Une mesure significative suppose une connexion au reseau
  public.
- Le verdict attendu est **des donnees comparatives**, pas un booleen
  « fingerprintable ou non ».

## Mesures de reference (2026-10-02)

Sessions collectées sur le réseau public réel (Windows, machine sous
charge : campagne fuzz + banc interop concurrents) :

### OnionBit idle — 20 min (`onionbit_idle.csv`, 10 745 lignes)

- ~15,8 Mo émis / ~13,1 Mo reçus → **~13 Ko/s up, ~11 Ko/s down**.
- 28 types de messages ; dominants : `remote_select` (DHT
  find-value, 8,5+4,9 Mo), `on_health` (4,8 Mo), `on_cell` (2,5 Mo —
  le noeud relaye le trafic tunnel des autres même au repos),
  similarity + introduction/puncture (discovery).
- Taille moyenne ~240 B/message. Aucun circuit propre READY (0/0) —
  le trafic observé est du service rendu au réseau (relai, DHT),
  pas de la maintenance de circuits.
- Signal notable : un daemon idle **sert deja de relais tunnel et de
  noeud DHT** — son empreinte n'est pas nulle.

### OnionBit transfert e2e — 10 min (`onionbit_e2e_active.csv`,
4 165 lignes ; downloader D du banc interop sens B, circuit lie
RP_DOWNLOADER, 8 Mio)

- ~142 Mo émis / ~149 Mo reçus → **~236 Ko/s par sens**.
- Quasi-totalité dans `on_cell` (4,97M messages, ~57 B en moyenne) :
  uTP encapsule dans les cellules, beaucoup de petits ACK.
- 4 circuits READY maintenus pendant le transfert.
- Le basculement de mixte discovery/tunnel → presque tout `on_cell`
  est un signal d'activité tres visible : un observateur du pair
  voit immediatement la difference idle/transfert.

### Tribler 8.4.3 — session publique (`target/fingerprint-tribler/`,
16:24 → ~17:18)

- API REST **inutilisable sous charge publique** : TCP accepte mais la
  boucle asyncio est saturee par ~100 relais (`too many relays (100)`
  en continu dans le log) — timeouts 25–120 s, confirmé sur deux
  sessions. Pas de contournement : `max_joined_circuits` est fixe a
  100 en dur cote pyipv8, et l'endpoint Rust ne persiste pas les
  statistiques en base.
- Collecte de repli : compteurs UDP systeme (`netstat -s`, toutes les
  10 s, ~120 echantillons) + stdout. **~1 000–1 300 datagrammes/s par
  sens** — trafic de relais vers le reseau reel. Le processus a
  disparu ~17:18 sans message d'arret (crash sous charge ou fermeture
  externe non determinee).
- Baseline grossiere uniquement : datagrammes/s systeme, sans
  ventilation par overlay ni confirmation que tout le trafic est
  tunnel.

### Mesh controle OnionBit + Tribler — 15 min
(`target/fingerprint-mesh-20261002-172000/`, `fingerprint_mesh.ps1`)

Topologie fermee loopback : A1 ancre+exit (IPv8 17787), A2/A3 relais,
D = daemon OnionBit mesure (API 8096), T = Tribler mesure (API 52198)
— memes communautes (ipv8 + tunnel + dht + content discovery),
`min_circuits=3` des deux cotes, aucun telechargement actif.

| Metrique (15 min) | OnionBit D | Tribler T |
|---|---:|---:|
| Endpoint total up | 3,34 Mo (~3,7 Ko/s) | 1,75 Mo (~1,9 Ko/s) |
| Endpoint total down | 3,13 Mo (~3,5 Ko/s) | 1,61 Mo (~1,8 Ko/s) |
| Circuits DATA READY | **0** | **3** (1 saut via A1) |
| Octets circuits | 0 | ~40 Ko up / ~21 Ko down |

- **Divergence 1 — construction proactive.** Tribler construit
  `min_circuits` des le demarrage meme sans telechargement. OnionBit
  ne construit que lorsqu'une lane anonyme existe (`anon_engine`
  paresseux → watchdog de circuits) : idle sans download anonyme =
  zero circuit propre. Ecart assume dans le code (`circuits_needed`
  par lane), mais c'est un signal de fingerprint comportemental et un
  biais de comparaison : pour un run a perimetre egal il faut creer
  une lane anonyme sur D (download anonyme ajoute via l'API).
- **Divergence 2 — volume idle ~1,9×.** Les 3,3 Mo de D sont ~100 %
  de churn discovery : 4 familles overlay (DiscoveryCommunity,
  DHTDiscoveryCommunity, ContentDiscoveryCommunity,
  TriblerTunnelCommunity) marchent chacune ~2 fois/s
  (~1 800 intros + punctures + reponses en 15 min). Attribution
  incertaine : mesh loopback (tous pairs en meme IP WAN → punctures)
  amplifie peut-etre les deux implementations differemment.
- **Limite Tribler confirmee** : `/api/ipv8/overlays/statistics`
  retourne `{"statistics":[]}` structurellement (endpoint Rust,
  `enable_community_statistics` no-op) — comparaison par msg_id
  impossible cote Tribler. Metriques communes retenues :
  `total_up`/`total_down` endpoint, sommes `circuits`/`relays`/`exits`,
  compteurs libtorrent, taille DB (`fingerprint_stats.ps1` etendu).

### Mesh controle avec lane anonyme — 15 min
(`target/fingerprint-mesh-20261002-183456/`, meme topologie,
`-WithAnonDownload` : magnet en stall `btih:00…01` ajoute en
anonyme 1 saut sur D et T — lane creee, `min_circuits` maintenu,
aucun pair/metadonnee disponible)

| Metrique (15 min) | OnionBit D | Tribler T |
|---|---:|---:|
| Endpoint total up/down | **328,7 / 328,9 Mo** (~365 Ko/s) | **1,66 / 1,49 Mo** (~1,8 Ko/s) |
| Circuits DATA READY | 3 (1 saut via A1) | 4 (1 saut via A1) |
| `on_cell` messages | ~5,80 M dans chaque sens (~6 400/s) | n/a (stats vides) |
| Octets dans circuits | 35,3 Mo up / 325,9 Mo down | ~56 Ko up / ~30 Ko down |

- **Signal de fingerprint majeur** : meme workload (« download
  anonyme en attente de metadonnees ») → OnionBit debite ~200× le
  trafic de Tribler. Un relais/exit observant la cadence distingue
  immediatement les deux implementations.
- Attribution : la lane anonyme OnionBit active la **DHT mainline
  a travers le tunnel** (`enable_dht=true`, socket uTP tunnelisee) —
  librqbit y fait son bootstrap/crawling contre le vrai reseau DHT
  via la sortie A1, et le `find_peers` de la metadata tourne en
  continu. Tribler route ses lookups de swarm anonymes via la
  `DHTDiscoveryCommunity`/hidden services, bien plus parcimonieux
  (et sa session libtorrent mesh a `dht=false`).
- Caveat de comparaison : la symetrie n'est pas parfaite
  (`dht=false` cote Tribler vs lane DHT forcee cote OnionBit) — mais
  le delta mesure est le comportement *par defaut* de chaque
  implementation, donc un fingerprint reel. A creuser : limiter le
  debit/agressivite DHT anonyme cote OnionBit est probablement
  aussi souhaitable d'un point de vue charge reseau.

### Interpretation preliminaire

- Les deltas up/down par msg_id sont exploitables ; la taille
  moyenne des cellules (~57 B) reflète le transport uTP encapsule.
- Rien d'anormal ne saute dans la distribution des types de messages
  idle — le daemon se comporte comme un membre overlay ordinaire.
- En mesh idle, l'empreinte d'OnionBit est **discovery pure**
  (~2× le volume Tribler, aucun circuit), celle de Tribler est
  **circuits + discovery** — l'activite de construction proactive de
  Tribler est le premier signal differentiel concret.
- En lane anonyme active, le rapport explose : **~200×** le volume
  Tribler, concentre dans `on_cell` (DHT mainline tunnelisee). Le
  couple « cadence de circuits au repos » + « volume d'un download
  anonyme en stall » est aujourd'hui la signature d'implementation
  la plus marquante.
- Reste : download public avec guards (critere de sortie consigne),
  puis validation workspace et decision guards par defaut.

## Attribution finale + mitigation (2026-10-02, post-mesure)

Attribution du signal ~6 400 `on_cell`/s — **par instrumentation des
types de cellules** (`cell_type_counts`), pas par hypothese : le
flood etait ~99 % de cellules `ping` (msg 6). Cause racine
protocolaire : `TunnelPong` etait un alias de `TunnelPing`, donc la
reponse au keepalive de circuit repartait etiquetee `ping` ; chaque
extremite OnionBit repondait a ce « ping » par un nouveau ping →
**tempete auto-entretenue a ~6 000 cellules/s** declenchee par le
premier `do_ping` (7,5 s apres le premier circuit READY). Les pairs
pyipv8 emettent un vrai `pong` : l'orage n'apparait qu'entre nœuds
natifs — c'est pourquoi l'interop Tribler n'avait rien montre.

Correction : vrai type `TunnelPong` (`msg_id=7`, trame `I, H`
identique). Resultat en mesh (run `fingerprint-mesh-20261002-214456`,
meme scenario magnet stall) : **~1 cellule/s residuelle** sur la
lane — le signal fingerprint a disparu (~5 000× sous la baseline).
Test de non-boucle ajoute (`ping_pong_ne_declenche_pas_de_tempete`).

Note de methode : l'hypothese initiale « amplification DHT reinjectee
par l'exit » etait plausible mais **fausse pour le volume observe** —
les compteurs d'exits (`inbound_accepted` ~20-40 par socket) et les
compteurs de sockets de lane (<256 datagrammes livres) ont montre
que le flux n'atteignait ni les sockets de sortie ni les sockets de
lane. L'instrumentation par type de cellule a tranche en un run.

Durcissement DHT / exit conserve (mesures preventives reelles,
independantes du flood — detail `docs/CHANGELOG.md`) :

- posture client-only par defaut sur la socket DHT tunnelisee
  (`anon_dht_client_only`) — les requetes entrantes sont ecartees a
  la frontiere ;
- budget de traitement des requetes entrantes dans librqbit-dht
  (`inbound_queries_per_second`, filet si client-only est desactive) ;
- plafond de debit sortant de la socket DHT de lane
  (`anon_dht_rate_pps` = 30 datagrammes/s, rafale 1 s, drop-tail) ;
- backoff exponentiel jittere des `get_peers` sans progres
  (`anon_dht_backoff_cap_secs` = 900 s) — un magnet sans swarm
  decroit vers ~1 vague/quart d'heure au lieu d'un regime permanent ;
- correction du double envoi de requete vendored (`request_one`
  envoyait deux fois) — -50 % de volume sur le chemin nominal ;
- conntrack de sortie (`exit_inbound_source_ttl_secs` = 300 s, table
  bornee a 2048 sources) : `exit_recv_data` ne reencapsule que les
  datagrammes non-IPv8 venant d'une destination deja contactee — le
  bruit WAN non sollicite n'entre plus dans le tunnel (IPv8 e2e
  exempte : create-e2e sans contact prealable).

L'ensemble est borne, teste et configure — mais le resultat
fingerprint de cette etape vient du correctif `TunnelPong`, pas de
la discipline DHT.
