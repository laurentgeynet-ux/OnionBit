# Étape 20 — Architecture de l'interface Flutter desktop

Plan d'architecture de l'interface graphique, réutilisant les patterns
de `C:\Emule-Sion-UI-UX\app`. Cible immédiate : **Windows desktop** ;
Linux/macOS ensuite. Android/iOS seront une interface de pilotage à
distance (même base, même API) — voir roadmap, étape 19 remplacée.

## Emplacement et stack

- Code dans `app/` à la racine du dépôt.
- Flutter 3.47 / Dart 3.13, Material 3.
- `flutter_riverpod` 3 (DI + état), `go_router` (navigation),
  `window_manager` + `tray_manager` (cycle de vie desktop),
  `shared_preferences` (préférences), `file_selector`,
  `intl` — même set de dépendances que l'app de référence.
- HTTP : package `http` (requêtes REST) + client SSE minimal fait
  maison (le flux `/api/events` est `text/event-stream` — parsing de
  `event:`/`data:` sur `StreamedRequest`, ~80 lignes, aucun package
  SSE tiers n'est nécessaire).

## Différence majeure avec l'app eMule

L'app de référence parle **JSON-RPC/WebSocket** ; `tribler-api` parle
**REST + SSE**. Le `RpcClient` devient donc `ApiClient` :

```
core/api/
  api_client.dart        — requêtes REST, gestion X-Api-Key,
                           erreurs {"error": {handled, message}}
  api_transport.dart     — abstraction (http réel / fake de tests)
  sse_client.dart        — flux /api/events : parse event:/data:,
                           reconnexion avec recul exponentiel
  events.dart            — topics Python typés (events_start,
                           download_state_changed, torrent_finished,
                           torrent_health_updated, …)
```

Le `reconnect_policy.dart` de l'app de référence est réutilisable tel
quel (SSE déconnecté → même politique de recul).

## Arborescence `lib/`

```
lib/
  main.dart
  app.dart                 — MaterialApp.router + bootstrap providers
  core/
    api/                   — ApiClient + SseClient (ci-dessus)
    config/                — AppConfig (hôte:port, clé API)
    di/providers.dart      — providers Riverpod racine
    layout/                — coquille (rail de navigation, barre d'état)
    logging/
    notifications/         — notifications Windows (torrent terminé…)
    platform/              — window_manager/tray, démarrage auto
    router/                — go_router
    storage/               — shared_preferences wrappers
    theme/                 — Material 3, seed color persisté
    widgets/               — widgets partagés
  features/
    dashboard/             — vue d'ensemble (stats tribler/ipv8)
    downloads/             — liste, détails, ajout magnet/torrent,
                             pause/resume/remove, fichiers, streaming
    search/                — recherche locale + distante
    channels/              — metadata (popular, health, tags)
    network/               — IPv8 : overlays, circuits, relais, swarms
    settings/              — settings tree + anonymat (hops)
    logs/                  — /api/logging + clierrors
    files/                 — navigateur /api/files + createtorrent
```

Chaque feature suit le découpage de la référence :
`data/` (DTO + repository impl) → `domain/` (entités + interface
repository) → `presentation/` (pages, providers, widgets).

## Correspondance features ↔ API

| Feature | Endpoints `tribler-api` |
| :--- | :--- |
| downloads | `GET/PUT /api/downloads`, `PATCH/DELETE /{ih}`, `/files`, `/stream/{i}`, `/trackers`, `clierrors` |
| dashboard | `/api/statistics/{tribler,ipv8}`, `PUT dirspace` |
| search | `/api/metadata/search/*`, `PUT /api/search/remote` |
| channels | `/api/metadata/torrents/{popular,health,{ih}/health}`, tags |
| network | `/api/ipv8/{overlays,tunnel/*}`, `/api/libtorrent/session` |
| settings | `GET/POST /api/settings`, `PUT /api/shutdown` |
| logs | `/api/logging`, `GET /api/downloads/clierrors` |
| files | `/api/files/{browse,list,create}`, `/api/createtorrent`, `/api/torrentinfo/*` |
| (temps réel) | `GET /api/events` SSE → invalidation Riverpod des providers |

## Cycle de vie desktop

- Fenêtre : `window_manager` (taille/position persistées, fermeture
  vers le tray optionnelle).
- `tray_manager` : icône de zone de notification + menu
  (afficher/quitter) — le daemon continue en tâche de fond.
- Connexion : `ApiClient` pointe `http://127.0.0.1:<port>` par
  défaut ; démarrage du daemon embarqué (process enfant) ou connexion
  à un daemon existant — décision V1 : **daemon enfant lancé par
  l'app** avec fallback « se connecter à un daemon existant ».

## Stratégie de tests

- Tests unitaires : DTO (JSON ↔ `downloads_endpoint` Python),
  `SseClient` (flux simulé), repositories avec `ApiTransport` fake.
- Tests widget : pages principales avec providers surchargés.
- Test d'intégration : `flutter test` contre un `tribler-daemon`
  `offline` lancé en process (script `scripts/test_app_e2e.ps1`).

## Sous-étapes

1. `flutter create app` + arborescence core/ + ApiClient/SseClient.
2. Dashboard + downloads (liste SSE-rafraîchie, ajout, pause/resume).
3. Search + channels + network (IPv8/tunnel).
4. Settings, logs, files, createtorrent.
5. Packaging Windows (`flutter build windows`) + script de release.
