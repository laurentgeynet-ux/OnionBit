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

## Thème et présentation

Décisions arrêtées le 2026-09-28.

### Thème — repris de `C:\Emule-Sion-UI-UX\app`, inchangé

Copie adaptée de `core/theme/app_theme.dart` + `theme_settings.dart` de
la référence : Material 3 via `ColorScheme.fromSeed`, seed bleu
`0xFF2F6FED` par défaut (la même palette `kAccentChoices` d'accents
utilisateur), `ThemeSettingsNotifier` persisté par
`shared_preferences` (seed + `ThemeMode` jour/nuit/**auto** —
`system` par défaut), tokens `AppSpacing`/`AppRadii`, cartes sans
élévation. Le `app_theme.dart` actuel (violet Tribler `0xFF8A2BE2`)
est à remplacer par cette version.

### Menus — sidebar fixe type Tribler (pas le `NavigationRail` de la référence)

La référence utilise `AdaptiveScaffold` (rail étroit / `NavigationBar`
bas en compact). Pour Tribler-Rust, l'utilisateur retient la
**présentation Tribler** : une vraie colonne latérale ~200 px avec
groupes dépliables, dérivée du pattern `AdaptiveScaffold` mais avec un
widget sidebar custom. En fenêtre très étroite (et pour le futur
pilotage mobile), repli sur `NavigationBar` bas — destinations
primaires seulement.

```text
┌──────────────────────────────────────────────────────────────────┐
│  Tribler-Rust   [🔍 Rechercher du contenu…]           ⚙  ─ □ ✕ │
├───────────────┬──────────────────────────────────────────────────┤
│ ＋ Ajouter     │  [Actions sélection : ▶ ⏸ 🗑]       [Filtrer…] │
│               │ ┌──────────────────────────────────────────────┐ │
│ Téléchargements│ │ Table : Nom, Taille, Progression, État,     │ │
│  ▾ Tous       │ │ ↓, ↑, ETA, Pairs, Anonymat (🛡)              │ │
│    En cours   │ └──────────────────────────────────────────────┘ │
│    Terminés   │ ┌──────────────────────────────────────────────┐ │
│    Actifs     │ │ Détails │ Fichiers │ Trackers │ Pairs        │ │
│    Inactifs   │ └──────────────────────────────────────────────┘ │
│  Rechercher   │                                                  │
│  Réglages     │                                                  │
│  Diagnostic   │  (overlays IPv8, circuits/relais/sorties,        │
│               │   swarms cachés, journaux, clierrors)            │
├───────────────┴──────────────────────────────────────────────────┤
│ ● Daemon connecté   ↓ xxx o/s (total)  ↑ xxx o/s (total)         │
│                    🛡 Lane anonyme : N circuits prêts / attente   │
└──────────────────────────────────────────────────────────────────┘
```

Points d'ergonomie retenus :

- **Anonymat honnête** : icône bouclier par téléchargement (plein =
  circuit prêt et trafic ; creux/orange = `anon_hops>0` mais aucun
  circuit `READY` → « en attente » ; absent = direct). La barre
  d'état agrège l'état de la lane via `/api/ipv8/tunnel/circuits` —
  jamais « connecté » sur la seule joignabilité du proxy SOCKS5.
- **Colonnes par défaut réduites** (Tribler en montre 16) :
  `Ratio`, `Last down/up`, `Added on`, `Completed on`, `Hops`
  réactivables par clic droit sur l'en-tête.
- **`Rechercher`** (pas « Découvrir ») : la page de recherche affiche
  les torrents **populaires** (`/api/metadata/torrents/popular`)
  comme contenu initial — l'écran n'est jamais vide. À la frappe
  (debounce ~300 ms), les résultats remplacent la liste : locaux
  (`search/local`) immédiats ; la recherche distante
  (`PUT /api/search/remote`) est lancée en parallèle avec indicateur
  « recherche en cours sur N pairs ». **Écart constaté
  (2026-09-28)** : le backend Rust intègre les `SelectResponse`
  directement dans `channel_node` **sans** pousser
  `remote_query_results` (contrairement à Python) — l'UI re-sonde
  donc `search/local` pendant la fenêtre de recherche (~10 s) et
  marque « réseau » les nouvelles entrées. Champ vidé → retour à la
  liste populaire. État vide propre prévu quand le daemon ne connaît
  encore aucun torrent (réseau non découvert).
- **Dialogue « Ajouter »** : champ magnet/URI ou fichier `.torrent`,
  aperçu `/api/torrentinfo`, destination, choix binaire
  « Direct / Anonyme (n sauts) » — `safe_seeding` coché
  automatiquement si anonyme (invariant déjà imposé par le backend).
- **Diagnostic** = tout ce qui est « interne » (pas d'écran dédié
  dans le parcours principal) : overlays, circuits, relais, sorties,
  swarms, journaux.

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
    platform/              — window_manager (tray/autostart : côté daemon, étape 29)
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

**Implémenté (première passe, 2026-09-28)** : `downloads`, `search`,
`settings`, `diagnostic` (fusion de `network` + `logs` : onglets
overlays/circuits/relais/sorties/swarms/pairs/journaux).

**Seconde passe (2026-09-28)** — toutes les fonctions du daemon :
file d'attente par téléchargement (queue_position, auto_managed,
limites individuelles, ratio de seed, recheck, move_storage,
inclusion/priorité par fichier, gestion complète des trackers) ;
diagnostic étendu (statistiques globales, points d'introduction
DHT/PEX, speed test de circuit) ; réglages complets (bande passante,
file d'attente `active_*`, seeding, tunnels `min/max_circuits` +
exit node, transports DHT/UPnP/NAT-PMP/LSD/uTP + proxy, watch
folder, flux RSS avec items, versioning, espace disque).
`dashboard`/`channels`/`files`/`createtorrent` restent à faire ou à
abandonner — la barre d'état et l'onglet Statistiques couvrent déjà
les compteurs globaux.

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

- Fenêtre : `window_manager` (taille/position persistées). Pas de
  fermeture vers le tray dans l'app : l'icône systray vit dans le
  **daemon** depuis l'étape 29 (menu « Ouvrir Tribler » relance
  l'UI) — un `tray_manager` Flutter ferait double emploi.
- Quitter l'UI ne ferme pas le daemon : il continue en tâche de fond
  sous son icône systray (« Quitter » du menu → arrêt propre).
- Connexion : `ApiClient` pointe `http://127.0.0.1:<port>` par
  défaut ; démarrage du daemon embarqué (process enfant) ou connexion
  à un daemon existant — décision V1 : **daemon enfant lancé par
  l'app** avec fallback « se connecter à un daemon existant ».
  **Implémenté** (2026-09-28) : `daemon_launcher` lance
  `tribler-daemon[.exe]` voisin de l'exe détaché si l'API ne répond
  pas, sonde jusqu'à 30 s ; `TRIBLER_DAEMON_EXE` surcharge le chemin
  en dev. `demarrer.cmd`/`arreter.cmd` supprimés (2026-09-28) —
  l'UI suffit pour lancer, le systray ou `PUT /api/shutdown` pour
  arrêter.

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
