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
