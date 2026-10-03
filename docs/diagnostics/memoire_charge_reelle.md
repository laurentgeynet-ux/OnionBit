# Mémoire — analyse de charge réelle (daemon public)

Diagnostic mené le 2026-10-03 sur une installation de production
(`onionbit-daemon.exe`, state dir utilisateur, réseau public Tribler,
~28 min d'uptime) comparée aux instances du banc `fingerprint_mesh.ps1`
(mesh loopback fermé, rôle idle). Question initiale : pourquoi le
daemon de production affiche ~110-160 Mo en mémoire quand les
instances de banc n'en montrent que ~12-16 Mo.

**Verdict : comportement normal pour la charge** — pas de fuite
caractérisée. L'écart est une facture de charge, pas un défaut.
Comparer un nœud public en hidden seeding à un nœud idle en loopback
n'a pas de sens.

## Mesures

| | Daemon prod (PID relevé) | 4 daemons banc (build debug) |
|---|---|---|
| Working set | ~159 Mo | ~40 Mo |
| **Mémoire privée** (= colonne « Mémoire » du GdT) | **~135 Mo** | **~16 Mo** |
| Handles | 886 | ~290 |
| Sockets | 10 TCP + 50 UDP | loopback uniquement |
| Espace virtuel | 10,7 Go (mmaps, réservations) | — |

La colonne « Mémoire » du Gestionnaire des tâches Windows correspond
à la **mémoire privée du working set** : les pages de fichiers mappés
(`allow_mmap`, partagées) n'y comptent pas mais apparaissent dans le
working set total.

## Charge réelle observée (API, instantané)

- **8 torrents en seeding** (~24 Go) avec **~100 pairs BitTorrent**
  connectés, ~0,8 Mbit/s d'upload effectif.
- **81 circuits tunnel** : 70 `IP_SEEDER` (hidden seeding — points
  d'introduction par torrent anonyme × lanes hops 1-3) + 11 `DATA`.
- **56 jambes de relais** servies aux autres nœuds (cap
  `tunnel_community/max_joined_circuits = 100`) et **44 sockets de
  sortie** UDP dédiées.
- **4 sessions librqbit** (hops 0, 1, 2, 3 — une session par
  profondeur d'anonymat, chacune avec DHT, socket uTP, listeners,
  pools de pairs).
- 326 pairs IPv8 connus, **12 063 torrents** en base de métadonnées
  (`onionbit.db` 23 Mo + WAL 31 Mo), ~32 700 messages `similarity`
  traités en 28 min.
- ~200 Mo de trafic IPv8 (relayé + découverte) en 28 min.

Le banc `fingerprint-mesh`, lui, est un mesh loopback à 4 nœuds sans
torrent — sa seule dépense est la baseline (binaire + Tokio + API +
communautés à vide).

## Décomposition de la mémoire privée (~135 Mo)

Ordre de grandeur par poste, relié aux allocations du code :

1. **Buffers uTP par pair** — poste dominant. uTP bufferise en espace
   utilisateur (contrairement à TCP et ses buffers noyau) : RX
   `RX_BUF_SIZE_PER_VSOCK_DEFAULT = 1 Mio`, TX 32 Kio → 1 Mio max par
   connexion, croissant avec la fenêtre de congestion
   (`vendor/librqbit-utp/src/constants.rs:19-27`). En seeding chaque
   pair sert des pièces : le TX se remplit jusqu'au BDP. ~100 pairs ×
   ~50 Kio-1 Mio ≈ **10-60 Mo**.
2. **Buffers wire par connexion** : `ReadBuf` 32 Kio + `write_buf`
   `MAX_MSG_LEN` ~16,5 Kio par pair ≈ **~5 Mo** pour 100 pairs
   (`vendor/librqbit/src/read_buf.rs:13`,
   `vendor/librqbit/src/peer_connection.rs:116`).
3. **4 sessions librqbit** (DHT, socket uTP, pools de pairs, workers
   disque chacune) ≈ **~20-40 Mo**.
4. **Tunnel** : par circuit, état crypto par saut (clés de session)
   + fenêtres anti-rejeu + files de cellules ; par sortie, `UdpSocket`
   dédiée + tâche de réception + `Hop`. ~180 endpoints actifs ≈
   **~5-15 Mo** (`crates/onionbit-tunnel/src/community.rs`,
   `Inner`/`ExitState`).
5. **SQLite + découverte de contenu** : cache de pages, statements
   préparés, désérialisation des métadonnées servies aux
   `similarity_request` ≈ **~5-15 Mo**.
6. **Baseline** : binaire + Tokio (stacks workers) + axum + tracing ≈
   les ~16 Mo mesurés sur le banc idle.
7. **mmap** (`libtorrent/allow_mmap = true`) : fichiers de seed mappés
   — ~23 Mo de working set *partagé* (dans les 159 Mo, hors privé) et
   les 10,7 Go d'espace virtuel (réservation d'adressage seulement,
   `vendor/librqbit/src/storage/examples/mmap.rs`).

À titre de référence, Tribler Python atteint typiquement 400-800 Mo
en charge ; ~135 Mo privés est sobre.

## Facteurs de croissance et caps

L'empreinte privée croît avec la charge, bornée par les caps de
configuration — une dérive *au-delà* de ces caps serait le signal
d'une fuite :

| Signal | Cap / borne | Source |
|---|---|---|
| Jambes de relais servies | `tunnel_community/max_joined_circuits` (100) | `daemon_config.rs` |
| Pairs BT | `peer_limit` = 64 **par session** (×4) | `onionbit-bittorrent/src/config.rs` (`DEFAULT_PEER_LIMIT`) |
| Buffers uTP | 2 Mio/connexion (RX 1 Mio + TX 1 Mio max), réglables via `libtorrent/utp_rx_buf_size` + `libtorrent/utp_tx_buf_max` | `vendor/librqbit-utp/src/constants.rs`, `EngineConfig::utp_socket_opts` |
| Circuits propres | `min/max_circuits` pour `DATA` ; `IP_SEEDER` dimensionné par nb de torrents anonymes × lanes × intro points | `daemon_config.rs`, `community.rs` |

Point de vigilance : le nombre de circuits `IP_SEEDER` doit
redescendre quand un torrent anonyme s'arrête — sinon fuite
d'état tunnel.

## Garde-fou

`scripts/mem_watchdog.ps1` échantillonne périodiquement la mémoire du
processus (`PrivateMemorySize64`, working set, handles) et les
compteurs de charge de l'API (circuits par type/état, relais joints,
sorties, pairs BT, taille DB) dans un CSV, puis signale :

- mémoire privée > `-MaxPrivateMB` (plafond absolu) ;
- dérive > `-WarnGrowthPct` **et** > `-WarnGrowthMB` vs le premier
  échantillon (croissance inexpliquée par les compteurs de charge,
  lisibles dans le même CSV) ;
- compteurs au-delà des caps (`-MaxCircuits`, `-MaxRelays`,
  `-MaxBtPeers`).

```powershell
.\scripts\mem_watchdog.ps1 -StateDir C:\path\to\state -DurationMin 120
```

Corréler `priv_mb` avec les colonnes de charge : une hausse de
`priv_mb` parallèle à `bt_peers`/`relays` est de la charge ; une
hausse de `priv_mb` à compteurs stables est suspecte.

## Réglage mémoire uTP (ajouté)

Les plafonds de buffers uTP sont configurables dans
`configuration.json` (restart-only) :

```jsonc
"libtorrent": {
  "utp_rx_buf_size": 262144,  // 0 = défaut librqbit-utp (1 Mio)
  "utp_tx_buf_max": 262144    // 0 = défaut (32 Kio initial → 1 Mio)
}
```

Appliqués à la session en clair **et** aux lanes anonymes via
`EngineConfig::utp_socket_opts` → `ListenerOptions::utp_opts` /
`TunnelUdpSockets::with_dht_policy`. Attention : le buffer RX est la
fenêtre annoncée — il borne aussi le débit d'une connexion
(`fenêtre / RTT` ; sur lane anonyme à RTT ~2 s, 256 Kio ≈ 128 Kio/s
par connexion). Ne pas baisser sous ~2× le BDP visé par connexion.
