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
