# Plan — Interface web (Flutter web)

Portage de l'interface Flutter desktop (`app/`) vers la cible **web**,
servie par `onionbit-daemon` lui-même. Rédigé le 2026-10-02 ; étapes
31-35 de `roadmap.md` (Phase 7).

## Contexte et constats

- `flutter build web` passe déjà (roadmap étape 20) et `app/web/`
  existe (squelette `flutter create`). La codebase est déjà préparée :
  les parties `dart:io` sont derrière des imports conditionnels
  (`daemon_api_resolver`, `daemon_launcher`, `desktop_shell`,
  `ui_log` — tous avec variante `_stub.dart`).
- **Bloquants web identifiés** :
  - `file_selector` (picker `.torrent` dans `add_download_dialog`,
    `automation_section`, `downloads_section`) : pas d'implémentation
    web — `MissingPluginException` à l'exécution ;
  - `desktop_drop` (`core/layout/drop_zone.dart`) : Windows/macOS/Linux
    uniquement, pas de drop HTML5 ;
  - `package:http` sur web = `BrowserClient` (XHR) : la réponse est
    **bufferisée en entier** — le flux SSE `/api/events` et le speed
    test de circuit (`getStreamedLines`) ne fonctionneront pas. Il
    faut `fetch_client` (Fetch API, streaming réel) côté web ;
  - `window_manager`/`tray_manager`/`dart:ffi` : déjà exclus par
    import conditionnel (`desktop_shell`), rien à faire ;
  - `daemon_launcher` : no-op web déjà en place (un navigateur ne
    lance pas de processus) ;
  - aucun service de fichiers statiques ni CORS dans `onionbit-api` —
    les exemptions `/ui`, `/static`, `/docs` de l'`ApiKeyMiddleware`
    Python n'ont pas d'équivalent (`auth.rs`) ;
  - `openPath` (explorateur natif), association `.torrent`, argv
    « Ouvrir avec » : concepts sans équivalent navigateur → à masquer
    via `kIsWeb`.

## Décision d'architecture (retenue)

**Le daemon sert le build Flutter web en same-origin.** L'utilisateur
ouvre `http://127.0.0.1:<port>/` et obtient l'UI complète, qui parle à
`/api/*` sur la même origine. C'est le modèle des exemptions `/ui` et
`/static` de l'`ApiKeyMiddleware` Python (Tribler embarque son UI).

Conséquences :

- **pas de CORS** : même origine → aucun en-tête `Access-Control-*` à
  ajouter, `onionbit-network-policy` et le refus de bind non-loopback
  restent intacts ;
- routes statiques **exemptées de `api_key_auth`** (parité Python) ;
  l'auth reste exigée sur `/api/*` — la clé est saisie une fois dans
  l'UI (persistance `shared_preferences` → `localStorage`) ou passée
  en `?key=`/cookie `api_key` (déjà acceptés par `auth.rs`) ;
- la protection anti-CSRF repose inchangée sur la clé : un site tiers
  ne peut ni lire `configuration.json` ni forger `X-Api-Key` sans
  preflight (bloqué sans CORS). Ne **jamais** embarquer la clé dans la
  page servie (cela neutraliserait l'auth).

Alternative écartée : héberger l'UI ailleurs + CORS. Cela exigerait un
mécanisme `api/cors_origins` (liste explicite, vide par défaut) — à
n'envisager que pour la boucle de dev (`flutter run -d web-server`),
via une étape optionnelle et un ADR, jamais par défaut.

Bénéfice secondaire : l'UI web servie par le daemon est aussi le
client de **pilotage à distance** sur navigateur mobile (étape 19
remplacée) — conditionné à l'exposition LAN optionnelle (Phase 7b,
hors scope initial car le daemon refuse le bind non-loopback).

## Étapes

### Étape 31 — Compatibilité de compilation et transports web

Verrouiller la cible et remplacer les dépendances desktop :

- `fetch_client` derrière un import conditionnel : `SseClient` et
  `getStreamedLines` instancient `FetchClient` sur web, `http.Client`
  sur desktop (`http_client.dart` stub/native, même pattern que les
  quatre existants) — sans ça, pas de SSE ni de speed test ;
- `file_selector` → abstraction `pick_file.dart` : desktop = inchangé ;
  web = `<input type="file">` via `package:web`/`dart:html` renvoyant
  des **octets** (compatible avec `ApiClient.putTorrent`, qui prend
  déjà `List<int>`) ;
- choix de dossier (destination par défaut, watch folder) : sur web
  ces chemins sont ceux du **daemon**, pas du navigateur → remplacer
  le picker local par une saisie texte ou un navigateur de
  `/api/files/browse` (étape 34 pour la version navigable ; saisie
  texte en étape 31) ;
- `desktop_drop` : `DropTarget` devient no-op sur web (stub) — la
  variante HTML5 arrive à l'étape 34 ;
- guards `kIsWeb` : `openPath` masqué/no-op (stub déjà prévu), pas
  d'argv `.torrent`, `daemon_launcher` déjà no-op ;
- `flutter build web` ajouté à `verify_all.ps1` (avec
  `flutter analyze` + `flutter test` si absents) pour verrouiller la
  cible à chaque étape.

### Étape 32 — Connexion et authentification web

- `AppConfig.baseUrl` web par défaut = `Uri.base.origin` (même
  origine que la page servie) — zéro configuration dans le cas
  nominal ; la résolution « daemon distant » manuelle reste possible
  (écran « Connexion daemon » existant) ;
- premier lancement / réponse 401 : dialogue de saisie de la clé API
  (l'utilisateur la lit dans `configuration.json` du daemon) ;
  persistance via `shared_preferences` (déjà en place) ; option de
  bookmark `?key=` (accepté par `auth.rs`) ;
- `ConnectionSettingsNotifier` : sur web, sauter l'étape
  `ensureDaemonRunning` (stub `null`, déjà correct) et privilégier
  l'origine servie ; `rediscover()` no-op web ;
- masquage des éléments propres au desktop (bandeau « daemon
  injoignable — relancer », lancement auto).

### Étape 33 — Service des statiques dans `onionbit-api`/`daemon`

- `tower_http::services::ServeDir` (ou `rust-embed` pour le binaire
  unique) : `GET /` → `index.html`, `GET /ui/*` + assets
  (`flutter_bootstrap.js`, `main.dart.js`, `assets/`, `icons/`,
  `manifest.json`) ; **fallback SPA vers `index.html`** pour les liens
  profonds `go_router` (ou `--base-href` + routage hash — décision à
  l'implémentation) ;
- exemption d'auth pour les chemins statiques **uniquement**
  (parité Python : `/ui`, `/static` hors `ApiKeyMiddleware`) — la
  couche `api_key_auth` s'applique par préfixe `/api` ; aucun
  affaiblissement sur `/api/*` ;
- config `DaemonConfig` : `api/web_ui_enabled` (défaut `true` quand un
  dossier UI est présent), `api/web_ui_dir` (défaut `<exe>/web` ou
  `state_dir/web`) ; pas de valeur en dur ;
- en-têtes de sécurité sur les statiques : `X-Content-Type-Options:
  nosniff`, `Cache-Control` (immutable pour les assets hashés
  Flutter, no-cache pour `index.html`) ; CSP raisonnable si simple ;
- tests : statique servi + fallback SPA + `/api/*` toujours 401 sans
  clé + traversée de chemin rejetée.

### Étape 34 — Adaptations UX web

- drop HTML5 (événements drag&drop navigateur) sur la drop zone →
  lecture d'octets → `putTorrent` ;
- navigateur de dossiers **côté daemon** : dialogue listant
  `/api/files/browse`/`list` pour « dossier de destination », « watch
  folder », `move_storage` — les chemins manipulés appartiennent à la
  machine du daemon ;
- streaming : `GET /api/downloads/{ih}/stream/{i}?key=…` ouvert dans
  un onglet du navigateur (lecture directe par le player HTML5 du
  navigateur quand le conteneur est supporté) ;
- notifications : API `Notification` du navigateur (permission
  demandée au premier événement) en plus des toasts existants ;
- `web/manifest.json` + `index.html` : titre/description réels
  (« A new Flutter project » à remplacer), icônes OnionBit, PWA
  « installable » (service worker Flutter déjà généré) ;
- revue responsive : `breakpoints.dart` (sidebar ≥ 600 dp /
  `NavigationBar` compact) suffisant — valider sur largeurs
  navigateur et mobile.

### Étape 35 — Packaging et parcours utilisateur

- `build_dist.ps1`/`build_release.ps1` : `flutter build web
  --base-href /` → `dist/<target>/web/` ; manifest mis à jour ;
  variante `rust-embed` (exe unique) si retenue à l'étape 33 ;
- systray : entrée « Ouvrir dans le navigateur »
  (`http://127.0.0.1:<port>/` avec port réel `http_port_running`) à
  côté de « Ouvrir Tribler » (UI desktop) ;
- docs : `app/README.md` + page utilisateur (où trouver la clé API,
  bookmark `?key=`, différence UI locale/UI web) ; CHANGELOG ;
- validation manuelle : parcours complet dans Chrome/Edge/Firefox —
  connexion, liste des downloads, ajout magnet + `.torrent`, SSE
  (mise à jour live + indicateur de connexion), réglages,
  diagnostic, speed test de circuit, streaming.

## Phase 7b — optionnelle, ultérieure

Hors scope V1, à ordonner si le besoin se confirme :

- exposition LAN : bind non-loopback derrière un réglage explicite +
  avertissements (le daemon le refuse aujourd'hui — invariant
  `onionbit-network-policy`) + HTTPS obligatoire ; ADR requis ;
- `api/cors_origins` (liste vide par défaut) pour la boucle de dev
  `flutter run -d web-server` ;
- mode « pilote à distance » mobile (étape 19 remplacée) : le build
  web sert déjà de client — reste l'auth renforcée si non-loopback.
