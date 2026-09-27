# Roadmap d'implémentation — Tribler-Rust-Torrent

Ce document est la **source de vérité de l'avancement**. Chaque étape est
cochée quand : implémentée + testée + validée manuellement + documentée +
commitée (cf. `AGENTS.md`, section "Workflow par étape"). Ne jamais
démarrer l'UI Flutter (étape 15+) avant que toutes les étapes backend
soient cochées.

Légende : `[ ]` à faire · `[~]` en cours · `[x]` terminée.

## Phase 0 — Fondations du dépôt

- [x] **Étape 0. Mise en place du dépôt.**
  Git local initialisé, licence GPL-3.0, `AGENTS.md`, workspace Cargo
  avec 12 crates squelettes (compilent, `cargo test`/`clippy -D warnings`/
  `fmt --check` passent), documentation d'architecture et de faisabilité
  initiale, script `scripts/verify_all.ps1`.

## Phase 1 — Moteur BitTorrent (via `librqbit`)

- [ ] **Étape 1. Fondations formats.** `tribler-format` : parsing
  `.torrent` (bencode via `librqbit-bencode` ou équivalent), liens
  magnet (BEP 9), calcul d'info-hash v1/v2. Tests contre des fichiers
  `.torrent` réels.
- [ ] **Étape 2. Crypto de base.** `tribler-crypto` : hachage SHA-1/SHA-256
  de pièces, génération de clés Ed25519/X25519 (tailles pyipv8
  low/medium/high). Tests contre des vecteurs connus.
- [ ] **Étape 3. Intégration `librqbit` et sessions de téléchargement.**
  `tribler-bittorrent` : démarrer/arrêter un téléchargement, suivre sa
  progression, DHT mainline, trackers HTTP/UDP. Test d'intégration :
  téléchargement réel d'un torrent de test (ex. contenu libre de droits)
  de bout en bout.
- [ ] **Étape 4. Schéma SQLite et migrations.** `tribler-db` : torrents
  connus, canaux, votes, réglages. Migrations versionnées
  (`SCHEMA_VERSION`). Tests avec base en mémoire.

## Phase 2 — Daemon minimal et API de contrôle

- [ ] **Étape 5. Session et Notifier.** `tribler-core` : `Session`
  (démarrage/arrêt ordonné), `Notifier` (bus d'événements interne),
  traits (ports) vers `tribler-bittorrent`/`tribler-db`.
- [ ] **Étape 6. API REST + WebSocket minimale.** `tribler-api` (axum) :
  endpoints `status`, `downloads` (list/add/remove/pause/resume),
  WebSocket de notification de progression. Bindé sur `127.0.0.1`
  uniquement. Documenter le mapping avec l'API Python dans
  `docs/reference_tribler/api_rest_mapping.md`.
- [ ] **Étape 7. CLI de pilotage minimal.** `tribler-cli` :
  `status`/`list`/`add`/`remove`, parle uniquement à `tribler-api`.
- [ ] **Étape 8. Premier daemon exécutable de bout en bout.**
  `tribler-daemon` assemble tout (sans IPv8) : config, logging
  `tracing`, démarrage propre/arrêt propre. Jalon : télécharger et
  suivre un torrent réel via `tribler-cli` de bout en bout.

## Phase 3 — Réseau d'anonymisation IPv8

- [ ] **Étape 9. Overlay IPv8 minimal.** `tribler-ipv8` : encodage/décodage
  des messages (format binaire pyipv8), bootstrap + peer discovery,
  une community triviale de test. **Jalon critique d'interopérabilité** :
  valider l'échange de messages avec un noeud Tribler Python réel
  (`pyipv8`) avant de continuer.
- [ ] **Étape 10. DHT overlay IPv8.** `tribler-ipv8` : implémentation du
  DHT overlay (distinct du DHT BitTorrent BEP 5), lookup de pairs.
- [ ] **Étape 11. Framework de communities complet.** `tribler-ipv8` :
  gestion de plusieurs communities simultanées, signatures Ed25519 sur
  tous les messages, gestion du churn/déconnexions.
- [ ] **Étape 12. TunnelCommunity : circuits et hidden seeding.**
  `tribler-tunnel` : construction de circuits en onion routing (1/2/3
  sauts), chiffrement AES-GCM par saut (`tribler-crypto`), hidden
  seeding, proxy SOCKS5 local. Jalon : téléchargement anonyme réel via
  un circuit construit contre le réseau Tribler existant.
- [ ] **Étape 13. Politiques de sécurité réseau et kill switch.**
  `tribler-network-policy` : anti-SSRF, politique des noeuds de sortie,
  kill switch atomique, garde-fous SOCKS5. Intégré dans
  `tribler-tunnel`/`tribler-bittorrent`.

## Phase 4 — Parité fonctionnelle et services secondaires

- [ ] **Étape 14. Services secondaires.** `tribler-core` : équivalents de
  `content_discovery` (découverte via canaux), `torrent_checker`
  (scrape santé des torrents), `rss` (abonnements), `watch_folder`
  (import automatique de `.torrent`).
- [ ] **Étape 15. Parité complète de l'API REST/WebSocket.** `tribler-api` :
  couverture de tous les endpoints nécessaires à une future UI (canaux,
  recherche, paramètres, statistiques de circuits). Mise à jour complète
  de `docs/reference_tribler/api_rest_mapping.md`.
- [ ] **Étape 16. Durcissement et tests de bout en bout.** Suite de tests
  d'intégration via `tribler-test-support` couvrant les scénarios
  critiques (téléchargement normal, téléchargement anonyme, redémarrage
  du daemon, migration de schéma DB, kill switch). Revue de sécurité des
  garde-fous réseau.

## Phase 5 — Packaging multiplateforme du backend

- [ ] **Étape 17. Builds desktop.** Windows x64/arm64, Linux, macOS :
  scripts de build reproductibles, vérification que `tribler-daemon`
  démarre et fonctionne sur chaque plateforme cible.
- [ ] **Étape 18. Étude dédiée mobile (Android/iOS).** Modèle d'exécution
  en arrière-plan (contraintes OS), avant toute tentative de build —
  peut nécessiter d'adapter `tribler-daemon` (service léger + réveils
  périodiques plutôt que daemon permanent).
- [ ] **Étape 19. Builds mobiles.** Android puis iOS, une fois le modèle
  d'exécution validé à l'étape 18.

## Jalon "backend terminé à 100 %"

Toutes les étapes 0 à 19 doivent être cochées et validées selon les
critères de `docs/plans/plan_faisabilite.md` §8 avant de passer à la
phase suivante.

## Phase 6 — Interface Flutter (ne démarre qu'après le jalon ci-dessus)

- [ ] **Étape 20.** Plan d'architecture Flutter dédié (nouveau document),
  réutilisant les patterns de style de `C:\Emule-Sion-UI-UX\app`, ciblant
  Windows/Linux/macOS/Android/iOS/Web, consommant exclusivement
  `tribler-api`.

---

## Notes de suivi

Ajouter ici, au fil de l'avancement, tout écart constaté par rapport au
plan initial (dépendance qui ne convient pas, étape scindée en deux,
risque IPv8 sous/sur-estimé, etc.), avec la date.

- 2026-09-27 : étape 0 terminée. Découverte de `librqbit` (ADR-0001) qui
  réduit fortement le risque des phases 1-2 par rapport à l'hypothèse
  initiale d'un moteur BitTorrent écrit entièrement à la main.
