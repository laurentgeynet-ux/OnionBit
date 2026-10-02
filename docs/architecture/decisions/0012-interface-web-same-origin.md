# ADR-0012 — Interface web Flutter servie en same-origin par le daemon

Statut : Acceptée (2026-10-02) — implémentée (étapes 31–35 de la
roadmap, plan `docs/plans/web_ui_plan.md`).

## Contexte

L'application Flutter `app/` cible Windows desktop ; l'objectif web
est de réutiliser le **même codebase** sans second backend — l'UI ne
parle qu'à l'API REST/SSE d'`onionbit-daemon`.

Deux options pour servir le build web :

1. **Serveur dédié / hébergement séparé** — impose du CORS, un second
   processus, et casse le modèle « un seul daemon loopback ».
2. **`onionbit-daemon` sert lui-même les statiques** sous `/` sur le
   même port que `/api/*` — exactement le modèle de l'
   `ApiKeyMiddleware` Python, qui exempte déjà `/ui` et `/static` de
   la clé API.

Trois contraintes techniques pesaient sur le portage :

- `package:http` en mode navigateur (`BrowserClient`, XHR) bufferise
  **intégralement** les réponses : le SSE `/api/events` et le speed
  test circuits (stream chunked) ne fonctionnent pas — il faut
  l'API Fetch du navigateur (`fetch_client`).
- `file_selector`, `desktop_drop`, `dart:io`, le lancement du
  processus daemon et la lecture de `configuration.json` sont
  impossibles ou absurdes dans un navigateur.
- L'auth API doit rester intacte : la page statique est publique,
  `/api/*` exige toujours la clé.

## Décision

**Same-origin** : `onionbit-api` monte les routes REST sous `/api`
(avec middleware clé, y compris un fourre-tout `/api/*` inconnu →
401, parité Python) et sert le build web sous `/` via `ServeDir` +
fallback SPA vers `index.html` — les statiques sont exemptés d'auth,
l'API jamais. Pas de CORS ajouté ; le daemon reste bind loopback
seul.

Configuration : `api/web_ui_enabled` (défaut `true`),
`api/web_ui_dir` (vide = détection `<exe>/web` → `state_dir/web` →
`app/build/web` en dev), CLI `--web-ui-dir`. En-têtes `nosniff` +
`Cache-Control` sur les statiques.

Côté Flutter, chaque couche plateforme est isolée par **import
conditionnel** (`dart.library.io`) : transport HTTP (`http` natif /
`fetch_client` web), picker de fichiers (`file_selector` / `<input>`
→ octets), sélecteur de dossiers (natif / `/api/files/browse` côté
daemon), drop zone (`desktop_drop` / événements HTML5), découverte
daemon (processus local / no-op + `Uri.base.origin`), notifications
(navigateur), ouverture d'URL.

La clé API est **injectée par le daemon** dans l'`index.html` servi
(`<meta name="onionbit-api-key">`, `api/web_ui_inject_key` = `true`
par défaut) : l'UI web se connecte sans saisie — équivalent de la
lecture de `configuration.json` par la GUI desktop. Sans risque hors
loopback : le daemon ne bind que sur `127.0.0.1` et la same-origin
policy empêche un site tiers de lire la réponse. Repli : saisie dans
le dialogue de connexion (bandeau « clé requise » sur 401) ou `?key=`
— mécanismes déjà acceptés par `auth.rs`.

Un lanceur `OnionBit Web.cmd`/`web-launch.ps1` dans `dist\` reproduit
le comportement du double-clic desktop : sonde l'API, démarre
`onionbit-daemon.exe --state-dir <dist>\state` si rien n'écoute
(jamais de double instance), puis ouvre le navigateur.

## Conséquences

- `flutter build web` fait partie de `verify_all.ps1` et de
  `build_dist.ps1` (`dist/web/`, auto-détecté par `<exe>/web`).
- Systray : entrée « Ouvrir dans le navigateur » quand une UI web est
  servie (port réel publié par le daemon).
- Une installation distante (mobile/LAN) reste hors scope : elle
  exigerait un bind non-loopback et une phase dédiée (Phase 7b du
  plan) — `onionbit-network-policy` n'est pas touchée.
- Sécurité : la surface publique s'élargit aux fichiers statiques du
  build (contenu figé, pas de donnée utilisateur) ; tout accès API
  reste derrière `X-Api-Key`/`?key=`/cookie.
