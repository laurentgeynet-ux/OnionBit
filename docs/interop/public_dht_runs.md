# Registre des runs — banc DHT publique / réseau Tribler réel

Chaque exécution de `scripts/interop_public_dht.ps1` est consignée ici,
**succès comme échecs**, avec les paramètres exacts et la route
observée. Banc **non déterministe** : les résultats dépendent des
pairs/relays/sorties réellement disponibles sur le réseau Tribler au
moment du run.

Topologie générale :

```text
downloader Rust ── (sauts libres, réseau Tribler réel) ──> sortie
Tribler réelle ──> swarm BitTorrent public
```

- Sélection : libre (`send_extend` standard — `candidates` du
  `created`, repli `EXIT_BT|RELAY`, alternates de retry). Aucun
  épinglage, aucun `required_exit`.
- Découverte : marche aléatoire sur le préfixe `a3591a6b`, bootstrapée
  par un `walk_to` vers `Tribler.exe` local (127.0.0.1:8090) — son seul
  rôle est l'introduction dans l'overlay.
- DHT : `router.bittorrent.com`, `dht.transmissionbt.com`,
  `router.utorrent.com`, `dht.aelitis.com` — requêtes routées dans le
  tunnel via `TunnelUdpSocket`.
- Intégrité : `progress_bytes` rqbit ne compte que les pièces dont le
  hash BitTorrent est vérifié.

## Run 2026-09-30 — 2 sauts — **OK**

- Magnet : `magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10`
  (Sintel, swarm public)
- Paramètres : `-Hops 2 -MinBytes 1048576 -WalkSeconds 30
  -DownloadTimeoutSec 240 -MaxCircuits 4`
- Découverte : 3 pairs tunnel connus, 2 flaggés exit, 3 relais
  (introductions reçues via Tribler.exe local)
- Circuit : **1er essai** — premier saut tiré librement
- Route observée (`verified_hops`, mids) :
  `["7a2fb4b77ec1fba71bc6b1a21d7ebe3df59a0198",
    "c2b2f2ece69b1b43dd0cd621231e56fa9cc8c8f6"]`
  → 2 pairs publics réels (aucun nœud contrôlé)
- TAP : trafic cellules via `192.42.116.241:36004` (premier saut réel)
- Octets vérifiés : **1 376 119** (≥ 1 Mio demandé) sur
  129 302 391 du torrent
- DHT : bootstrap public à travers le tunnel, pairs découverts
- Verdict : `INTEROP PUBLIC DHT OK`

## Run 2026-09-30 — 3 sauts (1re tentative) — **ÉCHEC : timeout**

- Même magnet. Paramètres : `-Hops 3 -MinBytes 524288
  -WalkSeconds 30 -DownloadTimeoutSec 240` (budget total ~750 s)
- Cause : le budget a été entièrement consommé par les tentatives de
  circuit (jusqu'à `hop_timeout × (hops+1)` par essai) sans atteindre
  un `READY` — le processus a été tué à expiration.
- Limite de l'outil constatée : les flux redirigés n'étaient écrits
  dans les logs qu'à la fin ; un timeout perdait le diagnostic.
  Corrigé : le script vide désormais les flux même après kill
  (`$stdoutTask`/`$stderrTask` écrits avant `exit 1`).

## Run 2026-09-30 — 3 sauts (2e tentative) — **OK**

- Même magnet. Paramètres : `-Hops 3 -MinBytes 262144
  -WalkSeconds 25 -DownloadTimeoutSec 200 -MaxCircuits 5`
- Découverte : 2 pairs tunnel, 1 exit flaggé, 2 relais (pool plus
  réduit que le run 2 sauts — l'overlay varie)
- Circuit : **1er essai** — premier saut tiré librement = Tribler.exe
  local (`38891a02d7dc61ed4b70179f3f5077f1010f5c5f`, mid de
  l'instance installée, présent dans le pool via le bootstrap)
- Route observée :
  `["38891a02d7dc61ed4b70179f3f5077f1010f5c5f",
    "ce9ff7b8f1e18509c543eabebaaa049fc8e8e09c",
    "a002e79f32790ff2774cec6dc1f40a3f927ca8ba"]`
  → Tribler.exe → relais public → sortie publique
- DHT : warnings `error in bootstrap: no successful lookups` suivis de
  retries puis pairs trouvés — variabilité normale des routeurs
  publics vus à travers une sortie réelle
- Octets vérifiés : **327 543** (≥ 256 Kio demandé)
- Verdict : `INTEROP PUBLIC DHT OK`

## Lecture des mids de route

- `38891a02d7dc61ed4b70179f3f5077f1010f5c5f` = `Tribler.exe` installé
  (mid stable — même identité que dans le banc épinglé
  `interop_tribler_relay.ps1`).
- Toutes les autres mids des runs ci-dessus sont des pairs du réseau
  public Tribler (non contrôlés).

## Ce que ces runs NE prouvent pas

- Pas de garantie de disponibilité : le réseau public varie (la
  première tentative à 3 sauts a échoué faute de circuit READY).
- Pas de tenue de session longue : 327 Kio–1,37 Mio démontrent la
  chaîne complète, pas la stabilité sur des heures.
- Pas de preuve que le dernier saut était *flaggé* exit — la route
  observée liste les mids vérifiés ; le rôle est inféré de la position
  et du fait que les données ont effectivement quitté le réseau.
