# Plan — Enrichissement global de l'app (UI Flutter + API)

Date : 2026-10-02. Demande utilisateur : suite des enrichissements
(`app_downloads`, `app_search`, `app_settings`, `app_sidebar` faits).
Points validés de la revue globale — mobile exclu (sera une interface
« télécommande » développée après validation de la version Windows).

Constats préalables : `EmptyState`/`ErrorState` existent déjà dans
`core/widgets/` ; la recherche globale avec autocomplétion FTS est
déjà dans `TopBar` ; Diagnostic a 10 onglets fonctionnels mais aucune
vue d'ensemble. Le plan vise donc les vrais trous.

Chaque étape = implémentation + `flutter analyze`/tests (ou
`verify_all.ps1` pour les étapes backend) + cochage ici +
`docs/CHANGELOG.md` + commit dédié. `vendor/` hors périmètre.

## Étape 1 — Palette OnionBit [x] (2026-10-02)

- `AppTheme.defaultSeedColor` → violet `#6C2EA6` (couleur du logo),
  accents cyan `#4FD8E0` en `tertiary` ; densité compacte confortable.
- Vérifier le rendu des éléments `primaryContainer` (sidebar pilules).

## Étape 2 — Diagnostic : onglet « Vue d'ensemble » [x] (2026-10-02)

- Premier onglet : cartes compteurs (overlays, circuits ready/total,
  relais, sorties actives, pairs tunnel, torrents, débits) alimentées
  par les providers existants, auto-refresh périodique (5 s).
- Santé globale : pastille verte/orange/rouge selon overlays présents
  et circuits READY.

## Étape 3 — Centre de notifications [x] (2026-10-02)

- `notificationsProvider` : liste session (titre, message, sévérité,
  horodatage) alimentée par les événements SSE (`torrent_finished`,
  erreurs de téléchargement) + `versions/check` (mise à jour dispo).
- Cloche dans `TopBar` avec badge non-lus et panneau déroulant
  (marquer tout lu, vider) ; `TorrentFinishedListener` alimente le
  centre en plus du snackbar.

## Étape 4 — Bannière « daemon injoignable » actionnable [x] (2026-10-02)

- La bannière existante gagne un bouton « Configurer… » ouvrant un
  dialogue adresse/port/clé API (réutilise `ConnectionSection` /
  `DaemonConfigProvider`) — onboarding de première connexion.

## Étape 5 — Persistance des préférences UI [x] (2026-10-02)

- `shared_preferences` : rail rétracté, filtres repliés, tri des
  tables Téléchargements/Rechercher, accent couleur/mode thème.
- Provider `uiPrefsProvider` chargé au démarrage, écrit à chaque
  changement ; providers existants initialisés depuis les prefs.

## Étape 6 — Vue compacte Téléchargements

- Bascule table ↔ liste dense (ListTiles avec % + débits) via
  bouton dans la barre d'outils ; choix persisté (étape 5).

## Étape 7 — SSE `settings_changed` (backend)

- `tribler-api` émet `settings_changed` après `POST /api/settings` ;
  l'UI écoute `daemonEventsProvider` et invalide
  `daemonSettingsProvider` → resynchronisation multi-clients et de
  l'éditeur avancé.

## Étape 8 — Tests widget des pages enrichies

- Tests widget : sidebar (pilules, badge erreurs, rail), réglages
  (dirty banner, défauts), recherche (badge en cours), downloads
  (tri/filtres) — cible : ~10 tests supplémentaires.
