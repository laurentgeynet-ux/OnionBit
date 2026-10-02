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

### Tribler 8.4.3 — session C

Non collectée : `Tribler.exe -s` joint bien le réseau réel (circuits
établis, `too many relays`) mais son API REST ne répond pas sous la
charge machine concurrente (timeouts >90 s même en warmup). À
refaire une fois la campagne fuzz terminée.

### Interpretation preliminaire

- Les deltas up/down par msg_id sont exploitables ; la taille
  moyenne des cellules (~57 B) reflète le transport uTP encapsule.
- Rien d'anormal ne saute dans la distribution des types de messages
  idle — le daemon se comporte comme un membre overlay ordinaire.
- Une analyse comparative Tribler reste necessaire avant toute
  conclusion sur le fingerprinting proprement dit.
