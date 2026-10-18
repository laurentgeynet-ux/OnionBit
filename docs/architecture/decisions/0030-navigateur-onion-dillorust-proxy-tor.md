# ADR-0030 — Navigateur `.onion` embarqué (DilloRust) : proxy Tor loopback, navigateur jetable

Statut : **Proposée** (2026-10-17) — jalons de décision 129-130
(arbitrage §8 + spike proxy) avant tout code.
Plan d'implantation : `docs/plans/roadmap_adr0030.md` (étapes 129-135).
Dépend de l'ADR-0028 v1 (runtime Tor sortant livré) ; complète la
trajectoire amorcée par l'ADR-0029 (adresses `.onion` devenues
cliquables). Extension hors fidélité Tribler (ADR-0020 §extensions,
ADR-0015) : Tribler n'a aucun équivalent.

## Contexte

**DilloRust** (`D:\Projet\dillorust`, dépôt externe autonome) est le
port sémantiquement fidèle du navigateur Dillo en Rust pur :
`#![forbid(unsafe_code)]` workspace-wide, décodeurs d'images pur Rust
(`png`, `jpeg-decoder`, `image-webp`, `resvg`), rendu
`winit`+`softbuffer`+`tiny-skia`, **aucun moteur JavaScript** — par
conception amont, pas par réglage. Le web `.onion` réel (forums,
annuaires, lanceurs d'alerte, moteurs type Ahmia/DuckDuckGo) est quasi
exclusivement du HTML/CSS statique, précisément pour rester accessible
sans JavaScript : l'adéquation est naturelle.

État réel vérifié (ROADMAP dillorust, 2026-10-17) : le moteur est
largement porté (net, html, css, dw, tables/OOF, images, services —
phases 1-5 et 7 cochées), mais **le navigateur navigable n'existe pas
encore** : phase 6 partielle (chrome `ui.cc`, menus/dialogs, `dillo.cc`
main non portés — le binaire actuel est le pilote 0.5 `minidoc` +
`file://`), phase 8 (DPI) et phase 9 (goldens/fuzzing/packaging) non
commencées. Le présent chantier est donc **conditionné** à la
production d'un binaire navigable côté dillorust ; les étapes OnionBit
(129-133) peuvent néanmoins démarrer, le contrat d'intégration étant
indépendant du shell navigateur.

Besoin produit : l'ADR-0029 fait apparaître des adresses `.onion`
dans la messagerie, les canaux et le coffre ; l'ADR-0028 rend les
résultats de recherche externe et les statuts de miroirs consultables.
Aujourd'hui, cliquer un `.onion` exige de quitter OnionBit pour un Tor
Browser externe — rupture de la bulle, fuite de contexte (l'utilisateur
copie-colle une adresse, parfois vers le mauvais navigateur). Un clic
doit ouvrir un navigateur **à l'intérieur de la zone de confiance**.

## Décision proposée

### 1. Positionnement honnête : lecteur `.onion`, pas « Tor Browser »

La promesse est précise et assumée :

- **Surface d'exploit minimale** : pas de moteur JS (ni JIT, ni WebRTC,
  ni canvas fingerprinting actif), `unsafe` interdit, parseurs
  tolérants mais bornés, rendu CPU simple.
- **Ce n'est pas le Tor Browser** : Dillo n'implémente ni HTML5 ni CSS
  moderne (≈2.1, pas de flexbox/grid) — une partie du web, y compris
  des sites « privacy-friendly », ne rendra pas correctement. Les
  défenses anti-fingerprinting de Tor Browser (letterboxing, UA/locale
  uniformes, isolation first-party du stockage, updater audité) sont
  un chantier séparé (§6), pas livrées par le moteur seul.

L'UI et la doc ne revendiquent jamais l'équivalence Tor Browser — le
composant est nommé « navigateur `.onion` » dans les chaînes
utilisateur.

### 2. Architecture : deux dépôts, un contrat — le proxy forward loopback

Le point dur est le transport. Trois faits mesurés :

1. `dillo-net` concentre déjà tout l'établissement de connexion
   (`dns.rs` → `addr_list` → `connect_task`/`TcpStream::connect`,
   `http.rs:958`) — règle d'architecture « connecteurs injectables »
   du port.
2. **Le mécanisme `http_proxy` est déjà porté** dans `http.rs` :
   URI absolue pour `http://`, tunnel `CONNECT host` pour `https://`,
   `no_proxy`, `Proxy-Authorization`, et l'env `http_proxy` prime sur
   `dillorc` (`HttpEngine::new`, `http.rs:332`). C'est la mécanique
   amont fidèle — aucune ligne de transport à écrire côté dillo.
3. `onionbit-tor` est le propriétaire unique de tout trafic Tor
   (ADR-0028) ; `TorRuntime` encapsule déjà `TorClient::connect`
   (`open_stream`) et `wrap_tls`.

Décision : **le daemon sert un proxy HTTP forward en loopback**
(`127.0.0.1:<port>`) fronté par `TorClient::connect`. Le navigateur
est un client proxy ordinaire — il reste « bête » : aucun code Tor,
aucune résolution DNS, aucune connaissance arti.

```
app Flutter ──POST /api/browser/launch {url}──▶ daemon
                                                 │ spawn
                                                 ▼
   dillo.exe  ──http_proxy=127.0.0.1:P──▶ TorProxyServer (onionbit-tor)
   (dillorust)      GET http://x.onion/  │   ├─ politique cibles (§4)
                    CONNECT h:443        │   ├─ auth token (§3)
                     │  │                │   ├─ isolation par origine (§3)
                     │  └─ octets ◀──────┴──▶ TorClient::connect → arti
                     └─ TLS rustls (https)      DataStream
```

Conséquences :

- **Un seul `TorClient`, un seul guard set, un seul bootstrap** — la
  lane permanente de l'ADR-0028 sert la recherche, les services
  `.onion` (ADR-0029) et le navigateur. La pire erreur serait un
  second client arti dans le binaire navigateur : double bootstrap,
  guard set divergent (signal de corrélation), et fusion des
  workspaces exigée.
- **Dillorust reste autonome** : son dépôt, sa ROADMAP, ses audits de
  parité et `PORTING.md` continuent leur cycle propre. Le contrat
  côté dillorust se réduit à : « honorer `http_proxy`, ne pas résoudre
  de DNS, profil éphémère (§5) » — dont les deux premiers sont déjà
  portés.
- **Fonctionne partout où le daemon tourne**, y compris le mode
  `external` (SOCKS5 d'un Tor système) — le proxy émet alors vers le
  SOCKS externe : comportement homogène.
- **Jamais de repli** : proxy injoignable ou politique refusée → la
  navigation échoue, point. Aucun fetch direct n'existe côté dillo
  (le système n'offre que le dialer proxy — cf. §5, confinement du
  DNS).

### 3. `TorProxyServer` dans `onionbit-tor`

Nouveau module, même crate (propriétaire unique de Tor), derrière la
feature `tor` :

- Listener TCP `127.0.0.1` uniquement (refus de toute adresse
  non-loopback à la config — même discipline que `--listen` daemon
  et `proxy_guard`).
- Deux formes de requête :
  - `CONNECT host:port` → cible validée (§4) → `open_stream` →
    `200 Connection Established` puis relais bidirectionnel borné ;
  - `GET http://hôte/chemin` (URI absolue) → idem, la requête est
    réécrite en forme origin avant envoi sur le `DataStream`.
  Toute autre forme (`OPTIONS *`, URI relative sans CONNECT) → `400`.
- **Bornes** : ligne de requête + en-têtes ≤ `max_header_bytes`
  (`httparse`, déjà dépendance), `max_conns` simultanées, timeout
  d'inactivité `idle_timeout_ms`, pas de `CONNECT` en chaîne.
- **Auth** : `Proxy-Authorization: Basic base64("onionbit:"+token)`
  exigée quand `require_auth` — token aléatoire par session du daemon,
  transmis au navigateur via l'env de lancement (jamais persisté,
  jamais logué). Un processus local tiers ne peut pas réutiliser la
  lane Tor sans le token. *(Jalon 129 : vérifier que dillo propage
  `http_proxyuser`/les credentials de l'URL proxy — le code porté
  montre `set_proxy_passwd` + `Proxy-Authorization` ; le chemin
  « userinfo dans l'URL `http_proxy` » est à confirmer au spike.)*
- **Isolation par origine** : quand `isolation="per_origin"`, le proxy
  maintient une map bornée `hôte cible → TorClient::with_prefs
  (StreamPrefs::set_isolation(token))` — les flux vers une même
  première partie partagent un circuit, jamais avec les autres
  origines ni avec la lane recherche/services (qui a son propre
  handle). ≈ `IsolateDestAddr` du SOCKS Tor.
- **Journalisation** : jamais d'URL, d'hôte cible ni de token dans
  `tracing` au-dessus de `debug` — même règle que les termes de
  recherche (ADR-0028 §5).

### 4. Politique de cibles — `TorTargetPolicy` inversée et durcie

L'allowlist du fetcher borné (ADR-0028 §3) ne convient pas : un
navigateur a une destination **non bornée a priori**. La politique du
proxy est donc inversée — tout est permis sauf ce qui est interdit :

- **`.onion` v3 valide** (validateur `format::onion`) → autorisé,
  ports {80, 443}.
- **Clearnet via exit** → `tor.browser_proxy.allow_clearnet_exit`,
  **défaut `false`**, cohérent avec le retrait `api_via_exit` de
  l'ADR-0028 : un site clearnet en HTTP voit son contenu par le nœud
  de sortie — l'option existe (navigation générale légitime sous Tor)
  mais n'est activée que par décision explicite de l'utilisateur,
  affichée comme telle dans les réglages. En HTTPS le contenu reste
  chiffré de bout en bout — seule la destination est visible de
  l'exit, ce qui est le fonctionnement normal de Tor.
- **Toujours refusés** : littéraux IP (v4/v6), loopback, privé/CGNAT,
  link-local, `file:`/`about:`/`data:`/tout schéma non-http(s) passé
  au proxy, `.onion` malformé — anti-SSRF `network-policy` appliqué
  au nom de cible **avant** tout `connect`. `no_proxy` dillo couvre
  le local par construction (le navigateur ne contacte jamais le
  proxy pour `file://`).
- **Aucun DNS système** : le proxy ne résout jamais — le hostname brut
  part vers `TorClient::connect` (résolution distante par le relais de
  sortie ou le rendez-vous onion). La fuite DNS est rendue
  structurellement impossible : il n'y a pas de chemin de code qui
  résolve.

### 5. Étanchéité du navigateur — exigences transmises au chantier dillorust

Le proxy garantit le transport ; l'hygiène navigateur relève du
dépôt dillorust (sa roadmap propre, ses propriétaires). L'ADR fixe le
contrat minimal — chaque point est vérifiable par les tests §roadmap :

- **DNS court-circuité** : en mode proxy, `dns.rs` ne doit jamais être
  invoqué pour une cible distante (le hostname passe dans la requête
  proxy, verbatim). `to_socket_addrs` ne se déclenche que pour le
  local.
- **Ressources locales uniquement hors proxy** : `file://`, `about:*`,
  `data:` servis en local ; tout `http(s)` passe par le proxy — y
  compris les sous-ressources (images, CSS, favicons, liens).
- **Profil éphémère « session »** : cookies, HSTS, historique, dicache
  — en mémoire ou dans un répertoire jetable supprimé à la fermeture ;
  **aucun fichier persistant** d'amont (`cookies.txt`, `hsts`) écrit en
  mode `.onion`, sauf téléchargement explicite de l'utilisateur.
  **Exception voulue — les bookmarks** : ils ne vivent plus dans
  `bm.txt` mais sont délégués au daemon (§6 « Services de profil ») :
  la trace existe, mais chiffrée et liée à l'identité, pas à la
  machine. L'historique, lui, reste **toujours éphémère** — les
  bookmarks sont une donnée voulue, l'historique est une empreinte
  subie. Les DPI persistants (phase 8 dillorust) seront confinés au
  même profil ; en v1 ils peuvent rester désactivés.
- **En-têtes normalisés** : UA neutre fixe (pas de version d'OS ni de
  build), `Accept-Language` fixe ou absent, `Referer` same-origin
  uniquement, ordre des en-têtes stable — réduire le fingerprinting
  passif là où il est gratuit de le faire. Le fingerprinting résiduel
  (polices installées, géométrie de fenêtre, timing) est documenté,
  pas prétendu résolu.
- **`https://*.onion`** : le certificat est vérifié par rustls comme
  d'habitude ; les rares `.onion` en HTTPS auto-signé affichent le
  dialog certificat amont — pas de contournement silencieux.
- **Pas de télémetrie, pas d'appel sortant hors proxy** : le binaire
  navigateur ne doit ouvrir **aucune** socket hors le proxy configuré
  — test d'étanchéité exigé (étape 134 : observation des flux du
  processus pendant une session).

### 6. API et UX

- `POST /api/browser/launch` `{url}` sous `api_key_auth` : valide le
  schéma (`http`/`https`) + la politique §4 + `tor Ready` → spawn du
  binaire `browser.binary_path` avec env `http_proxy=
  http://127.0.0.1:<port>` (+ credentials si `require_auth`) et l'URL
  en argument. Erreurs typées : `browser_not_configured` (503),
  `tor_disabled`/`tor_not_ready`/`bootstrap_failed` (503),
  `denied_target`/`invalid_url` (400). Le daemon possède port + token
  → l'app n'a jamais à les connaître.
- `GET /api/tor/status` gagne `browser_proxy {enabled, listen,
  conns_active}` — diagnostic et HUD.
- **App Flutter** : tout `.onion` affiché devient actionnable —
  messagerie et canaux (ADR-0029), résultats de recherche externe et
  chip de miroir (ADR-0028) : « ouvrir dans le navigateur » →
  `POST /api/browser/launch`. Le bouton est grisé honnête quand le
  navigateur n'est pas configuré (règle ADR-0028 §8 — jamais de
  joignabilité inexistante affichée).
- **Distribution** : `dillo.exe` (nom de bundle à figer — jalon 129)
  embarqué dans le bundle Windows du lanceur ADR-0018 ; chemins
  relatifs à l'arborescence portable. Linux : même binaire, même
  contrat.
- **Réglages** : sous-section « Navigateur » de la section Tor —
  toggle proxy, `allow_clearnet_exit` (défaut off, libellé explicite
  « le nœud de sortie voit le trafic non-HTTPS »), chemin du binaire,
  état `conns_active`. Appliqué à chaud via `swap_tor_stack` (le
  proxy vit et meurt avec le runtime).
- **Services de profil délégués au daemon** (v2 — requiert DPI, phase
  8 dillorust) : chez Dillo amont, bookmarks et cookies sont déjà des
  services dpi ; le port gardera le backend fichiers en standalone
  (fidélité), mais en mode suite le plugin bookmarks parle l'**API
  REST loopback** du daemon (`api_key_auth`) au lieu d'écrire
  `bm.txt`. Côté OnionBit : tables `browser_bookmarks` (titre, URL,
  horodatage) dans `onionbit-db` — chiffrées **gratuitement** par le
  codec ADR-0031, **portables** : même identité → mêmes bookmarks sur
  toute machine, synchronisables via le coffre `OBV1` comme les
  contacts. Règle de propriété : jamais d'ouverture directe du
  fichier DB par le navigateur — le daemon reste l'unique
  propriétaire (une fonctionnalité = un propriétaire ; deux writers
  SQLite inter-processus sur une base chiffrée = coordination
  fragile entre deux dépôts). La dépendance croisée se réduit à un
  contrat REST, pas à un fichier partagé ni à un crate importé.

### 7. Configuration

```json
"tor": {
  "browser_proxy": {
    "enabled": true,
    "listen": "127.0.0.1:0",
    "allow_clearnet_exit": false,
    "require_auth": true,
    "isolation": "per_origin",
    "max_conns": 64,
    "max_header_bytes": 16384,
    "idle_timeout_ms": 120000
  },
  "browser": {
    "binary_path": "bin/dillo.exe",
    "extra_args": []
  }
}
```

`listen: :0` = port éphémère choisi par l'OS au démarrage du runtime —
aucun port fixe à deviner/collisionner ; le port réel est publié par
`/api/tor/status` et injecté dans l'env du navigateur. Aucune valeur
en dur : tout seuil vit ici (règle AGENTS).

## Sécurité / vie privée

- **Fingerprinting passif résiduel** (liste honnête, non résolue en
  v1) : en-têtes HTTP (normalisés §5 mais distinctifs), polices
  installées détectables via métriques de rendu, géométrie de fenêtre,
  patrons de requêtes/timing, adresse d'exit par site si clearnet
  activé. Pas de JS ≠ pas de fingerprinting — le risque est réduit,
  documenté au THREAT-MODEL, jamais nié.
- **Processus locaux** : tout processus lisant loopback pourrait
  interroger le proxy — mitigé par `require_auth` (token de session,
  jamais persisté) ; à défaut (spike 129 négatif), repli documenté
  « loopback seul » = modèle Tor classique, acceptable mais plus
  faible.
- **Exit clearnet** : quand `allow_clearnet_exit=true`, le nœud de
  sortie voit les destinations et le HTTP en clair — affiché dans le
  réglage et au THREAT-MODEL ; défaut `false` préserve la ligne
  ADR-0028.
- **Persistance** : sans profil éphémère, `cookies.txt`/`hsts`/historique
  sur disque trahissent la navigation passée — exigence §5, vérifiée
  en étape 134.
- **Pas de secret croisé** : le token proxy n'ouvre ni l'API REST ni
  les services `.onion` de l'identité (ADR-0029) — portée limitée au
  forward.
- **Double usage de la lane** : le même `TorClient` porte recherche,
  services et navigation — circuits séparés par l'isolation §3, mais
  un adversaire positionné sur les gardes observe une activité
  agrégée ; assumé (un seul bootstrap est la propriété recherchée).

## 8. Questions ouvertes — à arbitrer (jalon 129)

1. **Auth proxy par env** : `http_proxy=http://user:pass@host/` →
   `Proxy-Authorization` automatique chez dillo ? Le code porté
   montre `set_proxy_passwd` + l'en-tête ; si le userinfo de l'URL
   n'est pas propagé, alternatives : `http_proxyuser` dans le
   `dillorc` du profil jetable, ou `require_auth=false` + loopback
   seul. Arbitrage au spike.
2. **Nom et forme du binaire** : `dillo.exe` amont ou
   `onionbit-browser` rebrandé ? Le pilote actuel est `minidoc` +
   `file://` — le premier vrai shell (phase 6 dillorust) fixera le
   point d'entrée CLI (`<binary> <url>`) auquel `browser.binary_path`
   + `extra_args` se conforment.
3. **`allow_clearnet_exit`** : défaut `false` proposé — confirmer ou
   passer à un tri-state (`off` / `https-only` / `on`) : refuser
   l'HTTP clair en sortie mais autoriser l'HTTPS vía exit est un
   compromis défendable (contenu toujours chiffré).
4. **Le navigateur sans daemon** : en v1 le proxy exige le daemon —
   usage toujours présent dans le produit (l'app parle au daemon).
   Un mode autonome (dillorust embarquant son propre arti) est
   rejeté (§Alternatives) — confirmer que l'exigence « autonomie » ne
   couvre pas le navigateur seul.

## Alternatives considérées

- **Dial in-process** (`dillo-net` ← `TorRuntime::open_stream`,
  esquisse initiale du cadrage Phase 21) : impose soit un second
  `TorClient` dans le binaire navigateur (double bootstrap, guard set
  divergent — signal de corrélation — et `onionbit-tor` comme
  dépendance croisée des deux dépôts), soit le navigateur compilé
  dans le daemon headless (incompatible : GUI dans un service
  headless/Docker, ADR-0024). Rejeté au profit du proxy — le code
  `open_stream`/`wrap_tls` existant reste la base interne du
  `TorProxyServer`.
- **SOCKS5 loopback** (modèle Tor Browser : firefox → `tor` SOCKS) :
  dillo ne parle pas SOCKS — ni amont ni port (seul `http_proxy`/
  `CONNECT` est porté, vérifié). Écrire un client SOCKS5 dans
  `dillo-net` serait du code neuf sans contrepartie amont — le proxy
  HTTP forward est strictement plus simple et déjà fidèle. Rejeté.
- **Vendoring / fusion des dépôts** (dillorust dans `crates/` ou
  `vendor/`) : couplage des cycles de vie — les audits de parité vs
  `D:\Projet\dillo` et `PORTING.md` ont leur propre rythme ; un port
  en cours dans `vendor/` devient inmaintenable. Rejeté ; le contrat
  proxy rend la fusion inutile.
- **WebView ou Tor Browser embarqué** (WebView2 + Tor externe) :
  réintroduit un moteur C++ massif et sa surface d'exploit — contredit
  la motivation même du projet. Rejeté.
- **Navigateur sans daemon** (arti embarqué dans dillorust) : double
  client Tor, dépendance croisée — rejeté (§8.4 à confirmer).

## Hors scope (rappels)

- Parité anti-fingerprinting avec Tor Browser (letterboxing, UA
  uniforme global, isolation du stockage par première partie) — v2
  éventuelle, listée honnêtement en §Sécurité.
- Navigation clearnet activée par défaut ; navigation FTP/gopher ;
  lecteur multimédia.
- Mobile (`app/` Android/iOS) : le contrat proxy fonctionnerait, mais
  le binaire dillo n'est pas portable tel quel — futur chantier.
- DPI servers dillorust (phase 8) : confinement dans le profil
  éphémère quand ils arriveront — pas d'intégration propre en v1.
  La seule délégation prévue est le service bookmarks vers l'API
  daemon (§6 « Services de profil », v2) ; tout plugin dpi qui ouvre
  une socket sortante reste banni du profil `.onion` (le proxy est le
  seul chemin réseau).

## Verdict du spike (étape 129, 2026-10-18) — GO

`onionbit-tor::proxy` (feature `tor`) : listener **loopback strict**
(`127.0.0.1:0` refusé hors-loopback), `CONNECT host:port` + `GET` URI
absolue réécrite en forme origin, parsing borné (`httparse`,
`max_header_bytes`), politique §4 **avant tout `connect`**
(littéraux IP, clearnet, `.onion` malformé, ports hors {80,443} →
403), `Proxy-*` jamais relayés (incl. `Proxy-Authorization`),
`max_conns` par sémaphore, `idle_timeout`. `open_stream` passé
`pub(crate)` — le proxy réutilise le `TorClient` unique (isolation
de circuits arti native), aucun second client.

**Preuves sur réseau réel** (sonde `proxy_probe`, arti standalone,
premier bootstrap hors mocks — ~20 s à chaud) :

- `curl -x http://127.0.0.1:<port> http://<ddg>.onion/` → **301**
  (redirection http→https du service — réponse réelle relayée) ;
- `curl --proxytunnel -x … https://<ddg>.onion/` → **200**, handshake
  TLS complet traversant le tunnel (ClientHello → Finished → données
  chiffrées relayées par `copy_bidirectional`) ;
- `curl --proxytunnel … http://<ddg>.onion/` (CONNECT :80) → 200 ;
- clearnet `http://example.com/` → **403** ; littéral `http://1.1.1.1/`
  → **403** ; forme origin `GET /` → **400** propre ;
- **zéro DNS système** : aucun `to_socket_addrs`/`getaddrinfo` dans le
  chemin proxy (grep + revue) — le nom est passé brut à
  `arti_client::TorClient::connect`, qui résout le `.onion`
  en interne ; le SOCKS5 externe ne résout pas non plus côté OnionBit.

**Promesse §5.2 mesurée** — un flux qui n'arrive pas au handshake est
coupé : la première exécution a exposé un bug réel — `write_all` sur
le `DataStream` arti peut *bufferiser sans émettre* ; sans
`flush()` explicite la requête n'atteint jamais le service, puis
`CircuitClosed` ~60 s plus tard. Correction : `flush()` après
écriture de la requête (et du `tail` CONNECT pipeliné) — le GET
`.onion` passe ensuite en une requête. Le `tracing::debug` des
compteurs de relais (octets, jamais de contenu) est conservé.

**Constat produit** : `OnionAddr::parse` (strict 56 chars) rejette
les sous-domaines `.onion` — `www.bbcnews…onion` (forme publiée par
BBC) obtient 403. Tor résout `*.<base>.onion` vers le même service ;
laisser un sous-domaine passer avec le `Host` intact est du ressort
de l'étape 131 (décision : permettre `*.onion` ou exiger la forme
canonique — à trancher à la productisation, documenté ici).

**Arbitrage §8 — les quatre questions** :

1. **Auth proxy** : le spike tourne loopback-seul sans auth (preuve
   suffisante). Pour 131 : `require_auth` optionnel par token de
   session via `Proxy-Authorization: Basic` — la propagation par
   `http_proxy=http://user:pass@…` reste à constater côté dillorust
   (étape 130) ; sinon `http_proxyuser`/`set_proxy_passwd` du
   dillorc éphémère. Jamais d'auth en clair hors loopback — le
   listener n'écoute de toute façon pas hors loopback.
2. **Nom du binaire** : pas de décision au spike — le contrat est le
   proxy, pas le CLI ; `onionbit-browser` (rebrandé) reste le nom
   retenu, la forme `<binary> <url>` se fixera avec la phase 6
   dillorust (`browser.binary_path` + `extra_args` en config).
3. **`allow_clearnet_exit`** : confirmé **tri-state** `off` /
   `https-only` / `on`, défaut **`off`** — le spike n'autorise que
   `.onion` ; un mode `https-only` (CONNECT :443 clearnet vía exit,
   GET :80 toujours refusé) est un compromis défendable à implémenter
   en 131 si souhaité, jamais par défaut (contenu chiffré mais
   métadonnées de destination en clair vers l'exit).
4. **Navigateur sans daemon** : **confirmé rejeté en v1** — le proxy
   vit dans `TorRuntime`, un seul `TorClient` (un seul bootstrap, un
   seul guard set ; un arti dans dillorust doublerait le signal de
   corrélation et la dépendance croisée). « Autonomie » couvre
   l'app, pas le navigateur seul.

## Verdict de la vérification dillorust (étape 130, 2026-10-18) — conforme

Trois tests filaires ajoutés à `dillo-net::http::tests` (stub TCP
loopback capturant les octets réels — commit dillorust `03e850f`) ;

le `HttpEngine` porté honore le contrat proxy de bout en bout :

- **`http://`** : forme absolue `GET http://hôte/chemin` émise vers le
  proxy (`use_proxy && !use_tls` dans `make_query_str`) — prouvé par
  une cible `inexistant.invalid` **irrésolvable** dont la requête
  arrive entière au stub ;
- **`https://`** : `CONNECT hôte:443` émis sur la socket proxy ; la
  réponse `200` (accumulée `https_proxy_reply`, branche 2F) déclenche
  `tls_wrap` sur **la même socket** — `ClientHello` observé après le
  CONNECT. Pas de `https.dpi` : TLS en-process (`TlsEngine`) — aucun
  plugin ne peut ouvrir sa propre socket et contourner le proxy
  (exigence §6 « profil onion » satisfaite par construction) ;
- **DNS** : en mode proxy seul l'hôte *proxy* est résolu — la cible
  voyage dans l'URI/le CONNECT, jamais dans `to_socket_addrs` ;
- **`no_proxy`** : bypass intact — requête en forme origine directe
  malgré un proxy `127.0.0.1:1` injoignable ;
- **`Proxy-Authorization`** : l'en-tête `Basic` émis **est** relayé
  quand `proxy_user`/`set_proxy_passwd` sont posés (dillorc
  `http_proxyuser`) — en revanche le userinfo de
  `http_proxy=user:pass@…` **n'est pas** propagé : l'init amont est
  commentée (*security problem, disabled* — non-bug adjudicé D1,
  audit A01).

**Conséquence pour §8.1 (auth)** : le token de session `require_auth`
peut être délivré via `http_proxyuser`/`set_proxy_passwd` dans le
dillorc éphémère — le canal existe et est prouvé ; l'userinfo d'URL
n'étant pas un canal disponible, il n'y a rien à supporter de ce
côté. Loopback-seul reste le défaut ; `require_auth` optionnel en 131.

**Détail de harnais** (fidélité) : l'armement du lecteur exige la
topologie capi amont — deux liens (`OpStart` branche 2 puis 1) et le
relais du `FD` remonté par `bfcb` (`capi.c` OpStart ×2 + relais
`OpSend "FD"` 1F→2B). Sans ce relais la réponse au CONNECT n'est pas
lue et `tls_wrap` ne démarre pas — le test le reproduit verbatim.

## Verdict de la productisation (étape 131, 2026-10-18) — fait

`onionbit-tor::proxy` passe du spike au produit ; `serve` retourne un
`TorProxyServer` (`local_addr` + `metrics`) au lieu du `SocketAddr`
nu. Quatre décisions tranchées :

1. **Sous-domaines `.onion` : permis** — le constat du spike se
   règle par `OnionAddr::parse_host` (ajout dans `onionbit-format`,
   `parse` canonique inchangé) : `*[label.]<56c>.onion` avec chaque
   label `[a-z0-9-]{1,63}` jamais de tiret en tête/queue, base v3
   strictement vérifiée (checksum SHA3-256 inclus), nom total
   ≤ 253. `x.onion.evil.com` reste refusé par le suffixe, les
   labels invalides (`so_us`, `-a`, `a..`) aussi. Fondement :
   `arti-client` identifie le service par les **deux derniers**
   labels et transmet le hostname complet au pair
   (`StreamInstructions::Hs { hsid, hostname }`) — les formes
   publiées `www.bbcnews…onion` fonctionnent nativement.
2. **Isolation par service, bornée** — un `IsolationToken` par
   **adresse du service** (hôte de premier niveau, sous-domaines
   inclus : `www.<base>` et `<base>` partagent le token — arti ne
   les distingue qu'en `hostname` transmis, pas en routage), façon
   first-party de Tor Browser. `open_stream_isolated` appelle
   `connect_with_prefs` + `StreamPrefs::set_isolation` — le même
   `TorClient` unique, jamais de second bootstrap. Map
   `Mutex<IsolationMap>` bornée à `isolation_max_entries` (défaut
   256) avec éviction FIFO : à saturation le plus ancien repart sur
   un token frais (circuits neufs — jamais de partage silencieux
   sans plafond, la borne contrôle mémoire et fan-out de circuits).
   Mode `external` : le token est ignoré — SOCKS5 ne transporte pas
   l'isolation ; un tor système applique sa propre politique
   (`IsolateDestAddr`) — documenté, pas masqué.
3. **Auth de session** — `ProxyConfig::auth_token` (`None` =
   loopback seul, défaut ; `Some` = `require_auth` effectif). Le
   *password* de `Proxy-Authorization: Basic <b64(user:pass)>` est
   le token, comparé en **temps constant** (`subtle::ConstantTimeEq`,
   même discipline que `onionbit-crypto`), le user est libre
   (DilloRust le fournit par `http_proxyuser`/`http_proxypass` —
   canal prouvé en 130). Échec → `407` + `Proxy-Authenticate`,
   compté dans `auth_failed_total`, **avant** la politique et avant
   tout `connect`. La valeur est capturée au parse puis jetée :
   jamais loguée, jamais dans les en-têtes relayés.
4. **Métriques exposées** — `ProxyMetrics` (`conns_active` via
   guard RAII, `conns_total`, `denied_total`, `auth_failed_total`),
   compteurs seuls (jamais d'hôte/contenu, ADR-0028 §5) — le
   matériau de `browser_proxy.conns_active` pour `/api/tor/status`
   (étape 132).

**Reporté** : `allow_clearnet_exit` tri-state (§8.3) non implémenté —
la politique reste `.onion`-only ; le tri-state est une extension de
`check_target` à décider avec la config de l'étape 132, jamais par
défaut.

**Tests** : 43 `onionbit-tor --features tor` verts — acceptation et
relais via SOCKS5 factice, refus politique sans socket (403),
`parse_host` (sous-domaines valides acceptés / malformés refusés),
auth 407 puis relais avec token, isolation (même origine → même
token, origines distinctes → tokens distincts, éviction bornée),
`conns_active` monté/descendu ; clippy `-D warnings` et fmt propres.

## Verdict du câblage daemon + endpoint de lancement (étape 132, 2026-10-18) — fait

Le proxy sort du laboratoire : le daemon le porte, l'API l'expose et
le déclenche. Cinq décisions tranchées :

1. **Politique de cibles = propriété de `onionbit-network-policy`** —
   la validation navigateur (`check_browser_target`) vit dans le
   crate anti-SSRF, compilable sans la feature `tor` : `check_target`
   de `proxy.rs` délègue, le handler `/api/browser/launch` vérifie la
   même fonction — une seule règle, deux points d'application.
2. **`allow_clearnet_exit` tri-state effectif, testé** —
   `ClearnetExit::{Off, HttpsOnly, On}` (défaut `Off`, valeurs
   inconnues rejetées au parse config) distingue `CONNECT` de `GET`
   absolue : `Off` = `.onion` seul ; `HttpsOnly` = clearnet
   uniquement en `CONNECT :443` (GET clearnet et `:80` toujours
   refusés — le compromis §8.3) ; `On` = clearnet `:80`/`:443` via
   exit Tor. Littéraux IP, plages privées/loopback/CGNAT/link-local
   et `.onion` malformés restent refusés dans **tous** les modes —
   l'effet de chaque valeur est testé, pas seulement parsé.
3. **Cycle de vie greffé au runtime** — `CoreSession` détient le
   slot `BrowserProxySlot { server, auth_token }` ; spawn à la
   création et re-spawn à chaque `swap_tor_stack`/`swap_tor_runtime`
   (le vieux serveur est droppé, son `JoinHandle` avorté). Le proxy
   ne démarre que si `tor.enabled && browser_proxy.enabled` ; un
   échec de bind est logué en warn et reflété par `listen=null` dans
   le status — jamais de blocage du bootstrap ni du démarrage.
4. **`GET /api/tor/status` expose `browser_proxy`** —
   `{enabled, listen, conns_active}` ; `listen` est l'adresse
   **réellement liée** (port éphémère résolu quand `127.0.0.1:0` est
   configuré), `conns_active` provient du handle `ProxyMetrics`
   vivant — jamais de copie figée, jamais de token.
5. **`POST /api/browser/launch` — déclencheur, pas exécuteur** —
   corps `{url}` strict : le chemin du binaire et `extra_args`
   viennent **exclusivement** de `tor.browser.*` en config (aucun
   `args` piloté par l'API — pas de surface d'exécution de commande).
   Erreurs typées : `invalid_url`/`denied_target` (400),
   `browser_not_configured`/`tor_disabled`/`tor_bootstrapping`/
   `tor_not_ready`/`bootstrap_failed` (503). **Token jamais en
   argv** : `require_auth` génère un token hex 256 bits par
   incarnation du proxy, livré au navigateur via le dillorc
   éphémère `<state_dir>/browser/.dillo/dillorc`
   (`http_proxypass`, extension documentée — le profil entier est
   redirigé par `env HOME=<state_dir>/browser` ; `http_proxy` passe
   aussi en env) — sous Windows les argv sont lisibles par les
   processus du même utilisateur, le token n'y figure donc jamais.
   L'URL validée reste le dernier argument (contrat
   `<binary> <url>`) ; jamais d'URL ni de token dans les logs.

**Tests** : 17 `onionbit-network-policy` verts (tri-state × méthode ×
famille d'hôte, normalisation, refus transverses), 43 `onionbit-tor`,
115 `onionbit-api` — dont les nouveaux : status expose
`browser_proxy` (activé avec adresse liée réelle / absent),
launch → 503 `tor_bootstrapping`/`browser_not_configured`,
400 `invalid_url` (schéma, `ftp`, `javascript`), 400 `denied_target`
(clearnet/IP/port hors plage en `Off`), 200 avec spawn réel du
binaire de test (`--list`) + dillorc éphémère vérifié
(`http_proxy`, `http_proxyuser`, pas de `http_proxypass` sans auth).
La feature `tor` de `onionbit-core` est unifiée en dev-deps de
`onionbit-api` — le runtime reste un stub, le listener proxy est
réel. Clippy `-D warnings`, fmt propres.

**Reste pour 133** : lecture effective de `http_proxypass` côté
DilloRust (le pref est écrit, ignoré en warn tant que le parseur ne
le connaît pas — amont n'a que `http_proxyuser` + dialog), contrat
browser côté UI Flutter, garde-fous réseau/process du bundle (134).

### Verdict étape 133 (2026-10) — `http_proxypass` lu + contrat UI Flutter

**DilloRust** (`dillorust@e1c656d`) : pref `http_proxypass` ajouté à
`Prefs`/`parse_option` — extension assumée, absente amont (qui n'a
que `http_proxyuser` et lit le mot de passe par dialog
`a_UIcmd_get_passwd`, impossible en spawn headless par le daemon).
La valeur sème la cellule partagée `proxy_passwd` **avant** la
factory de moteurs : chaque `HttpEngine` part authentifié
(`set_proxy_passwd` → `Proxy-Authorization: Basic`), le dialog de
démarrage saute quand le pref est renseigné. Clé absorbée par le
parseur tolérant — un dillorc OnionBit reste lisible par un Dillo
amont (warn + skip). Déviation documentée dans `docs/PORTING.md`
(lignes `prefsparser` et `dillo.cc`).

**Flutter** : sous-section « Navigateur (.onion) » dans Réglages →
Réseau — toggle `tor.browser_proxy.enabled` (immédiat, hot-swap),
politique `allow_clearnet_exit` tri-state avec hint honnête par
valeur (« l'exit Tor voit la destination » en `https-only`, « le
HTTP part en clair vers l'exit » en `on`), chemin `tor.browser.
binary_path`, pastille d'état `listen`/`conns_active` depuis
`GET /api/tor/status → browser_proxy`. Les hôtes `.onion` de
l'allowlist deviennent actionnables (icône `open_in_new`) →
`POST /api/browser/launch` sur `http://<hôte>/`, erreurs traduites
par `error.code` (`browser_not_configured`, `tor_not_ready`,
`denied_target`…) — jamais le message brut.

**Tests** : prefs `parse_file` couvre `http_proxypass` +
`http_proxyuser` ; 109 `dillo-browser` verts ; 118 tests Flutter dont
`network_section_tor_test` étendu (toggle immédiat, hint de sortie,
lancement allowlist → URL + snackbar) ; `flutter analyze` et
`check_i18n.ps1` propres.

**Reste pour 134** : étanchéité réseau/DNS/process du bundle
(fuites hors proxy, durcissement du profil éphémère).
