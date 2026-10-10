# OnionBit en conteneur Docker (ADR-0024)

Image headless du daemon `onionbit-daemon` : seedbox, nœud relais/exit
IPv8 24/7, NAS, serveur domestique. L'UI Flutter desktop n'est **pas**
dockerisable, mais le build web (`app/build/web`) est embarqué dans la
variante `final-webui` et servi par le daemon en same-origin
(ADR-0012) — client complet dans le navigateur.

## Build

```bash
# daemon seul (~150-250 Mio)
docker build --target final -t onionbit:dev .

# daemon + web UI Flutter embarquée (stage de build supplémentaire)
docker build --target final-webui -t onionbit:dev-webui .
```

Le `Dockerfile` est multi-stage : `rust:bookworm` compile (deps
tierces en couche cachée : `vendor/` + manifests + stubs —
obligatoire car les `[patch.crates-io]` par `path` exigent `vendor/`
dès `cargo fetch`), `debian:bookworm-slim` exécute sous l'utilisateur
non-root `onionbit` (UID **10001**).

## Les trois modes d'exposition de l'API

L'API de contrôle **bind `127.0.0.1` en dur** — le daemon refuse de
démarrer sur une adresse non-loopback (HTTP et HTTPS). Ce n'est pas
une limitation à contourner mais la politique de sécurité : `docker
run -p 8085:8085` ne joindra **jamais** le service (le NAT de Docker
aboutit sur `eth0`, pas `lo`).

| Mode | Comment | Accès API | Quand |
| :--- | :--- | :--- | :--- |
| **Réseau hôte** (recommandé) | `network_mode: host` ou `--network host` | `http://127.0.0.1:8085` depuis l'hôte | toujours, sur hôte Linux — zéro config, UPnP et P2P directs |
| **Bridge + forwarder** | sidecar `network_mode: "service:onionbit"` | port publié | accès depuis d'autres machines du réseau — voir avertissements ci-dessous |
| **Interne** | aucune exposition | `docker exec onionbit onionbit-cli status` | pilotage ponctuel sans exposition |

### Réseau hôte (recommandé)

```bash
docker run -d --name onionbit \
  --network host \
  -v ./data:/data \
  onionbit:dev-webui
```

`127.0.0.1:8085` du conteneur = loopback de l'hôte : la politique est
intacte, les ports BitTorrent (45000 tcp+udp) et IPv8 (UDP) sont
directement adressables, l'UPnP peut mapper les ports sur la box.

### Bridge + forwarder (exposition réseau)

Le loopback est **propre à chaque conteneur** : le forwarder doit
partager le namespace réseau du daemon — `network_mode:
"service:onionbit"` (un sidecar sur le réseau bridge ne peut pas
joindre le `127.0.0.1` du daemon, et l'IP bridge serait de toute
façon refusée par la garde loopback). Voir `docker-compose.yml` :

```yaml
forwarder:
  image: alpine/socat
  network_mode: "service:onionbit"
  command: "tcp-listen:8085,fork,reuseaddr tcp:127.0.0.1:8085"
```

Avec nginx à la place : **`proxy_buffering off;` obligatoire** sur
`/api/events` — le flux SSE tamponné fige l'interface.

> ⚠️ **`api/web_ui_inject_key=false` obligatoire.** La web UI injecte
> la clé API en `<meta>` de `index.html` pour l'auto-connexion — sûr
> en loopback, mais tout client joignant le port publié récupère la
> clé d'administration. Poser la clé à `false` dans
> `configuration.json` (l'UI demandera alors la clé à la connexion),
> et idéalement ajouter une authentification en amont (reverse proxy,
> Basic Auth, allowlist IP).

## Volumes et permissions

```bash
mkdir -p data && chown -R 10001:10001 data
docker run -d --network host -v ./data:/data onionbit:dev-webui
```

Le daemon écrit tout sous `/data` : `--state-dir /data/state` →
`data/` voisine `/data/data` (convention ADR-0018 — les chemins
persistés sont des specs `@state/`/`@public/` portables, aucun chemin
machine ne fige). Un seul volume couvre `configuration.json` (clé
API), la base SQLite, l'identité, les logs et les téléchargements.

- **Bind mount hôte** : le dossier doit appartenir à `10001:10001`
  (UID du conteneur) — piège n°1, sinon `Permission denied` sur
  `state/`.
- **Un volume = un conteneur** : le verrou d'instance (`fs2`)
  garantit l'unicité par `state_dir` — deux conteneurs sur le même
  volume : le second sort silencieusement.
- **`read_only: true`** est supporté (voir `docker-compose.yml`) :
  toutes les écritures sont confinées à `/data`, `tmpfs` sur `/tmp`
  pour `tempfile`.

## Ports

| Port | Config | Protocole | Note |
| :--- | :--- | :--- | :--- |
| 8085 | `api/http_port` | TCP | API + web UI — **loopback uniquement**, jamais publié directement |
| 45000 | `libtorrent/port` | TCP + UDP | pairs BitTorrent + uTP/DHT ; sonde `port..=port+10` si occupé |
| `ipv8/interfaces[UDPIPv4].port` | `--ipv8-port` | UDP | overlay IPv8 ; **à fixer** pour la publication bridge |
| SOCKS5 lanes | `socks_listen_ports` | TCP | internes aux tunnels anonymes — jamais publiés |

`libtorrent/upnp=false` en mode bridge (SSDP ne traverse pas le NAT
Docker) ; laisser `true` en host-network.

## Nœud relais / exit IPv8

Le trafic **sortant** (UDP/TCP P2P, circuits sortants) traverse le
NAT bridge sans problème. En revanche un nœud servant de **relais ou
d'exit** doit être **joignable en entrée** : publier le port IPv8 UDP
en bridge (`-p <port>:<port>/udp` avec `--ipv8-port` fixé), ou
préférer le host-network qui rend le nœud directement adressable.

## Clé API et pilotage

```bash
# CLI dans le conteneur — ONIONBIT_STATE_DIR=/data/state est
# pré-positionnée (etape 89), aucun argument requis :
docker exec onionbit onionbit-cli status
docker exec onionbit onionbit-cli list
docker exec onionbit onionbit-cli add "magnet:?xt=..."

# Cle API (pour un client externe en mode forwarder) :
docker exec onionbit cat /data/state/configuration.json | grep '"key"'
```

## Healthcheck, signaux, logs

- `HEALTHCHECK` = `onionbit-cli status` (API réelle + auth ; `curl /`
  répondrait 404 sans web UI).
- `STOPSIGNAL SIGTERM` : `docker stop` déclenche l'arrêt propre
  complet (`session.stop()` + drain) en quelques secondes —
  implémenté à l'étape 89 (`SignalKind::terminate`).
- Logs : `docker logs onionbit` (stdout) et fichiers sous
  `/data/state/logs/` (`onionbit.log` du run courant).
- `init: true` dans le compose (PID 1 correct : signaux relayés,
  zombies réapés).

## Compose

`docker-compose.yml` : mode host-network actif, variante bridge +
forwarder commentée.

```bash
docker compose up -d --build
docker compose logs -f
docker compose down          # SIGTERM -> arret propre
```

## Dépannage

| Symptôme | Cause | Correctif |
| :--- | :--- | :--- |
| `Permission denied` sur `/data/state` | bind mount appartenant à un autre UID | `chown -R 10001:10001 ./data` |
| API injoignable en `-p 8085:8085` | bind loopback — attendu | host-network ou forwarder `service:` |
| Conteneur `unhealthy` sans web UI | — | non applicable : healthcheck = cli, pas HTTP |
| Web UI sert `index.html` sans connexion auto | `inject_key=false` (correct en exposition) | saisir la clé API dans l'écran de connexion |
| `docker stop` long / SIGKILL | — | impossible : handler SIGTERM (étape 89) — vérifier les logs |
