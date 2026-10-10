# ADR-0024 — Image Docker du daemon (déploiement headless conteneurisé)

Statut : Proposée (2026-10-10). Partiellement implantée — étapes
89-92 et 94 livrées, l'étape 93 (validation e2e sous docker) reste
ouverte ; le statut passera à Acceptée après elle. Plan
d'implantation : `docs/plans/roadmap_adr0024.md` (Phase 16,
étapes 89-94).

## Contexte

OnionBit vise des usages « nœud toujours allumé » : seedbox, relais
IPv8/exit de tunnel 24/7, NAS, serveur domestique. Le daemon est déjà
un binaire unique `onionbit-daemon` qui assemble core + BitTorrent +
IPv8 + tunnels + API — le conteneur Docker est le format naturel de
distribution pour ces cibles.

Ce qui joue déjà en faveur :

- **Rust quasi pur** : rustls (pas d'OpenSSL), `rusqlite` `bundled`
  (compilateur C au build seulement), librqbit vendored, aucune
  dépendance système exotique. Le seul prérequis externe du build est
  `git` pour les dépendances `bitdaemon-*` épinglées par `rev`
  (`Cargo.toml` `[workspace.dependencies]`).
- **Support unix complet** : tout le code Windows (tray, autostart,
  console, `embed-resource`) est isolé derrière `cfg(windows)` ;
  `installed_state_dir` suit déjà XDG (`$XDG_DATA_HOME/onionbit`).
- **Headless fonctionnel** : sans `--first-run-gate`, l'identité est
  auto-générée ; `--no-tray`/`headless` existent ; l'arrêt propre
  passe par `PUT /api/shutdown` ou Ctrl-C.
- **UI web same-origin** (ADR-0012) : le build Flutter web servi par
  le daemon donne un client complet sans GUI — exactement le modèle
  des images « -nox » des clients torrent (qBittorrent-nox…).
- **État auto-contenu** : `--state-dir <d>/state` concentre
  `configuration.json` (clé API), SQLite, identité, logs ; `data/`
  devient voisine de `state/` automatiquement
  (`PathRoots::for_state_dir`, ADR-0018) → un seul volume suffit.

Les contraintes réelles identifiées :

| Contrainte | État du code | Impact Docker |
| :--- | :--- | :--- |
| API de contrôle verrouillée **loopback en dur** | `main.rs` refuse toute adresse non-loopback (`is_loopback()` → `ExitCode::FAILURE`) ; `https.rs` idem (`HttpsError::NotLoopback`) ; `proxy_guard` impose un SOCKS5 loopback | `docker run -p` ne peut PAS atteindre un bind `127.0.0.1` interne au conteneur |
| `api/web_ui_inject_key=true` par défaut | injecte `api_key` en `<meta>` de l'`index.html` servi | sûr en loopback ; **fuite de la clé API** vers quiconque joint le port si exposé au réseau |
| Pas de handler SIGTERM | seules sources d'arrêt : Ctrl-C (SIGINT), tray, `/api/shutdown` | `docker stop` → SIGTERM ignoré → SIGKILL après timeout → pas de graceful shutdown |
| `libtorrent/upnp=true` par défaut | SSDP multicast ne traverse pas le NAT bridge Docker | inopérant (mais inoffensif) en mode bridge ; à documenter |
| Ports P2P sondés | `libtorrent/port` (défaut 45000, sonde `port..=port+10`), `ipv8/interfaces[UDPIPv4].port` | ports à épingler + publier en mode bridge |
| Secrets persistés | `configuration.json` (clé API), `identity/`, `secondary_key.pem`, vaults `OBV1` | doivent vivre sur un volume, jamais dans l'image |

## Décision

### 1. Image multi-stage `rust:bookworm` → `debian:bookworm-slim`

Builder : `git` (deps `bitdaemon-*` épinglées) + `cc` (rusqlite
bundled) sont déjà présents dans l'image `rust` officielle ;
`cargo build --release --locked -p onionbit-daemon -p onionbit-cli`
(`onionbit-cli` embarqué pour `docker exec … onionbit-cli`).

Runtime : `debian:bookworm-slim` + `ca-certificates`
(`rustls-native-certs` est dans le `Cargo.lock` — requis pour les
trackers HTTPS et webseeds) + utilisateur non-root `onionbit`
(UID 10001, fixe pour les volumes — documenter
`chown -R 10001:10001 ./data` pour les bind mounts hôte, sinon
`Permission denied` sur `/data/state`). Pas de `curl` : le
healthcheck passe par `onionbit-cli` (décision 4). Pas de
`scratch`/musl en v1 : le coût de portage des deps C vendored et la
perte de debuggabilité ne se justifient pas (à réévaluer si la
taille devient un enjeu).

### 2. La garde loopback de l'API reste **absolue** — pas de flag d'ouverture

Conformément à la règle « ne jamais affaiblir
`onionbit-network-policy` », aucun `--listen-any` ni relâchement du
contrôle `is_loopback()` n'est introduit : **l'exposition est un
choix de déploiement, jamais un comportement du binaire**. Trois
modes documentés, par ordre de préférence :

- **Réseau hôte** (`network_mode: host`, hôtes Linux) — mode
  recommandé : `127.0.0.1:8085` du conteneur = loopback de l'hôte,
  politique intacte, ports P2P et UPnP fonctionnels, zéro
  configuration réseau ;
- **Bridge + forwarder** — relai TCP exposé vers `127.0.0.1:8085` :
  le daemon garde son bind loopback. Sous Docker, chaque conteneur a
  son propre loopback : le sidecar (socat ou nginx) doit donc
  **partager le namespace réseau** du daemon —
  `network_mode: "service:onionbit"` dans le compose
  (`tcp-listen:8080,fork → tcp:127.0.0.1:8085`). Si nginx est choisi,
  `proxy_buffering off;` est **obligatoire** sur `/api/events` : le
  SSE serait sinon tamponné et l'UI figée.
  `api/web_ui_inject_key=false` **obligatoire** dans ce mode (la
  `<meta>` d'`index.html` livrerait la clé API à tout client joignant
  le port) + authentification en amont fortement recommandée (reverse
  proxy) ; ports P2P publiés par `-p` (`libtorrent/port` TCP+UDP,
  port IPv8 UDP) ;
- **Interne** — aucune exposition : `docker exec <ctr> onionbit-cli`
  parle au loopback du conteneur.

### 3. Deux ajustements de code préalables : SIGTERM + `ONIONBIT_STATE_DIR`

- **SIGTERM** : `docker stop` envoie SIGTERM (ignoré aujourd'hui →
  SIGKILL après le timeout). Ajout de
  `tokio::signal::unix::signal(SignalKind::terminate())` aux sources
  d'arrêt de `wait_shutdown_sources` (cfg unix — le chemin Windows
  est inchangé). Même séquence que Ctrl-C : `session.stop()` → drain
  axum → sortie propre. `STOPSIGNAL SIGTERM` explicite dans le
  Dockerfile.
- **`ONIONBIT_STATE_DIR`** : `onionbit-cli` ne connaît que
  `--state-dir` (défaut `.onionbit`) — un `docker exec <ctr>
  onionbit-cli status` trouverait un `configuration.json` absent et
  échouerait en 401. Ajout de `env = "ONIONBIT_STATE_DIR"` à
  l'argument (clap : activer la feature `env` dans le workspace) ;
  le Dockerfile pose `ENV ONIONBIT_STATE_DIR=/data/state` → le CLI
  (healthcheck inclus) marche sans argument.

### 4. Layout du conteneur : un volume `/data`, web UI optionnelle

- `VOLUME /data` ; `ENTRYPOINT ["onionbit-daemon", "--state-dir",
  "/data/state"]` → `data/` voisine = `/data/data` (convention
  ADR-0018, aucun chemin machine persisté grâce aux specs `@root/…`) ;
- `CMD` = flags additionnels (`--ipv8-port`, `--bootstrap`, …) ;
- stage optionnel `flutter` (build `app/build/web`) →
  `/opt/onionbit/web`. L'auto-détection ne couvre que
  `<exe>/web` et `<state_dir>/web` : l'`ENTRYPOINT` de l'image
  « webui » porte donc `--web-ui-dir /opt/onionbit/web`
  explicitement (build-arg `WITH_WEBUI`) ;
- `HEALTHCHECK` : **`onionbit-cli status`** — teste l'API réelle
  (port publié + clé lue dans `configuration.json`), contrairement à
  `curl /` qui répond 404 quand la web UI est désactivée
  (route statique non montée dans `router.rs`).

### 5. Configuration recommandée conteneur (documentée, pas forcée)

- `libtorrent/upnp=false` en mode bridge (SSDP ne traverse pas le
  NAT Docker) ; laissé `true` en host-network où il reste utile ;
- ports épinglés : `api/http_port` (8085), `libtorrent/port`,
  `--ipv8-port` — la sonde `port..=port+10` rend la publication
  bridge imprévisible si le port de base est pris : documenter la
  plage ou imposer le port via le pare-feu du conteneur ;
- **rôle relais/exit IPv8** : le trafic sortant (UDP/TCP P2P) passe
  le NAT bridge sans problème, mais un nœud voué au relais ou à
  l'exit doit être **joignable en entrée** — publier le port IPv8
  UDP en bridge, ou préférer le host-network qui rend le nœud
  directement adressable (documenté dans `docs/docker.md`) ;
- `headless=true` / `--no-tray` explicite (no-op unix, mais garde le
  comportement si la config voyage depuis un state_dir desktop).

### 6. Secrets et `.dockerignore`

`configuration.json` (clé API), `state/identity/`, les vaults `OBV1`
et la base SQLite ne vivent que sur le volume `/data` (permissions
`700`) — jamais copiés dans l'image. `.dockerignore` racine exclut :
`target/`, `dist/`, `app/build/`, `.onionbit/`, `state/`, `data/`,
`*.key`, `*.pem`, la CI ne voit que `crates/`, `vendor/`,
`Cargo.toml`/`Cargo.lock`, `app/` (sources), `assets/`, `docs/`.

### 7. Livrables et tags

`Dockerfile` + `.dockerignore` + `docker-compose.yml` (exemple
host-network actif, variante bridge commentée) à la racine ;
`docs/docker.md` : modes d'exposition, volumes, récupération de la
clé API, healthcheck. Tags locaux `onionbit:<version>`/`onionbit:latest`
— pas de publication registry automatisée en v1 (job `publish` de
`ci.yml` → ghcr.io réservé à une étape ultérieure).

### Non adoptés

- **Flag d'écoute non-loopback** : contredit directement la règle
  AGENTS.md et le modèle de menace (l'API de contrôle ne sort jamais
  du loopback *du processus*).
- **Forwarder intégré au binaire** : doublon de fonction
  (socat/nginx le font mieux) et surface d'attaque ajoutée au
  processus privilégié.
- **Image musl/`scratch`** : reportée (cf. décision 1).
- **GUI Flutter / `onionbit-launcher` dans l'image** : headless par
  définition ; la web UI couvre le besoin client.

## Conséquences

- **Positif** : déploiement serveur reproductible (`docker run` /
  compose), nœud relais/exit opérationnel en une commande, web UI
  complète sans GUI ; le modèle de sécurité loopback est préservé
  intégralement — aucun assouplissement de `network-policy`.
- **Négatif / dette** : le mode bridge+forwarder déplace la
  responsabilité de l'exposition sur l'opérateur — `inject_key=false`
  est un prérequis documenté, pas forcé par le code ; l'image doit
  être reconstruite à chaque bump des `rev` `bitdaemon-*` ;
  `installed_state_dir` XDG reste court-circuité par `--state-dir`
  (comportement voulu, documenté).
- **Interop/filaire** : aucun écart — packaging uniquement.
