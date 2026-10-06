# Plan de faisabilité — OnionBit

Date de rédaction : 2026-09-27.
Statut : validé pour démarrage du backend (étape 0 terminée).

## 1. Objectif

Porter le daemon **Tribler** (client BitTorrent avec réseau d'anonymisation
propriétaire IPv8/TunnelCommunity, https://github.com/Tribler/tribler,
Python + aiohttp) vers un **daemon Rust natif**, exposé via une API
REST + SSE (Server-Sent Events) locale, avec une future interface **Flutter**
multiplateforme (Windows x64/arm64, Linux, macOS, Android, iOS, Web) qui ne
sera développée qu'une fois le backend validé à 100 % sur ses
fonctionnalités clés.

## 2. Analyse du logiciel de référence

Sources officielles étudiées : `<Tribler sources checkout> (env `TRIBLER_SRC`)`
(dépôt principal + sous-module `pyipv8`).

| Composant Python | Emplacement | Rôle | Taille approx. |
| :--- | :--- | :--- | :--- |
| `tribler.core.session` / `components.py` / `notifier.py` | `src/tribler/core` | Cycle de vie du daemon, bus d'événements | — |
| `tribler.core.libtorrent` | `src/tribler/core/libtorrent` | Sessions de téléchargement (wrapper autour de `libtorrent` C++), `.torrent`/magnet | 5 fichiers |
| `tribler.core.database` | `src/tribler/core/database` | Métadonnées torrents, canaux, votes (Pony ORM + SQLite) | 6 fichiers |
| `tribler.core.restapi` | `src/tribler/core/restapi` | API REST aiohttp + notifications SSE (`text/event-stream`) | 11 fichiers |
| `tribler.core.tunnel` | `src/tribler/core/tunnel` | `TunnelCommunity` : circuits onion routing, hidden seeding | 4 fichiers |
| `tribler.core.content_discovery` | `src/tribler/core/content_discovery` | Découverte de contenu via canaux | 4 fichiers |
| `tribler.core.torrent_checker` | `src/tribler/core/torrent_checker` | Vérification de la santé des torrents (scrape trackers/DHT) | 5 fichiers |
| `tribler.core.socks5` | `src/tribler/core/socks5` | Proxy SOCKS5 exposé par les tunnels | 4 fichiers |
| `tribler.core.rss` | `src/tribler/core/rss` | Abonnements RSS vers torrents | 2 fichiers |
| `tribler.core.watch_folder` | `src/tribler/core/watch_folder` | Import automatique de `.torrent` depuis un dossier surveillé | 2 fichiers |
| `pyipv8` | `pyipv8/ipv8/` | Protocole overlay IPv8 : peer discovery, communities, DHT overlay, attestation, crypto | plusieurs milliers de lignes |
| `ui/` | `src/tribler/ui` | Frontend React/TS consommant l'API REST/SSE | — |

**Constat clé** : l'architecture Python sépare déjà backend (daemon +
API REST/SSE) et frontend (React web). C'est exactement le schéma
visé (daemon Rust + Flutter), ce qui valide l'approche de découplage.

**Découverte importante** : Tribler utilise déjà un crate Rust,
[`ipv8-rust-tunnels`](https://github.com/Tribler/ipv8-rust-tunnels)
(LGPL-3.0), mais uniquement pour accélérer le **plan de données** des
tunnels (relais de paquets chiffrés) — le **plan de contrôle** (protocole
IPv8, construction de circuits, crypto, discovery) reste en Python. Il
n'existe donc **aucune implémentation Rust complète du protocole IPv8**
à ce jour : c'est officiellement le morceau le plus dur du projet, pas
seulement une intuition.

## 3. Écosystème Rust disponible (recherche effectuée le 2026-09-27)

| Besoin | Solution Rust existante | Verdict |
| :--- | :--- | :--- |
| Bencode, `.torrent`, protocole peer-wire, DHT mainline BEP 5, uTP, trackers HTTP/UDP | [`librqbit`](https://github.com/ikatson/rqbit) (Apache-2.0, ~1600★, actif, découpé en sous-crates `librqbit-core`, `librqbit-bencode`, `librqbit-dht`, `peer_binary_protocol`...) | **Réutiliser**, ne pas réécrire (cf. ADR-0001) |
| Protocole IPv8 complet (overlay, discovery, communities, DHT overlay, attestation) | Aucune implémentation Rust complète connue (`ipv8-rust-tunnels` ne couvre que le plan de données des tunnels) | **Porter depuis `pyipv8`**, risque élevé assumé (cf. ADR-0002) |
| Crypto Ed25519/X25519 | `ed25519-dalek`, `x25519-dalek` (mûrs, maintenus par le Dalek project) | Réutiliser |
| SQLite | `rusqlite` (mode `bundled`, cohérent avec le choix fait dans eMule-Rust) | Réutiliser |
| API REST + SSE | `axum` (déjà choisi dans eMule-Rust, cohérence d'écosystème) | Réutiliser |

Cette découverte change fortement l'estimation de risque/effort par
rapport à un portage "tout à la main" : le moteur BitTorrent de base
(BEP 3/5/9/10, peer-wire, tracker) devient une intégration de dépendance
plutôt qu'une réécriture de protocole binaire — le risque se concentre
presque entièrement sur IPv8/TunnelCommunity et sur la parité
fonctionnelle de l'API REST/DB.

## 4. Décisions d'architecture arbitrées avec l'utilisateur

| Décision | Choix retenu | ADR |
| :--- | :--- | :--- |
| Moteur BitTorrent | Moteur Rust pur, en s'appuyant sur `librqbit` plutôt qu'un FFI vers `libtorrent-rasterbar` (C++) — nécessaire pour un portage mobile/web réaliste | ADR-0001 |
| Périmètre IPv8/anonymat | Inclus dès la V1 du daemon (pas repoussé en V2) | ADR-0002 |
| Licence du projet | GPL-3.0-or-later, héritée de Tribler | ADR-0003 |
| Structure du dépôt | Workspace Cargo multi-crates, inspiré de la structure du projet **eMule-Rust** de l'utilisateur | ADR-0004 |
| UI | Flutter, développée **après** validation complète du backend ; réutilisation des patterns de style de `the reference Flutter UI project` | — (hors périmètre backend actuel) |
| Plateformes cibles | Windows x64/arm64, Linux, macOS, Android, iOS, Web | — |

## 5. Analyse de faisabilité par composant

| Composant | Risque | Complexité | Commentaire |
| :--- | :--- | :--- | :--- |
| `onionbit-format` (bencode/.torrent/magnet) | Faible | Faible | Largement couvert par les sous-crates de `librqbit` |
| `onionbit-crypto` | Faible | Faible-Moyenne | Primitives disponibles (dalek) ; le travail est surtout de reproduire les tailles de clé pyipv8 |
| `onionbit-bittorrent` (intégration `librqbit`) | Faible-Moyenne | Moyenne | Adapter l'API `librqbit::Session` aux besoins Tribler (notifications, routage optionnel via SOCKS5 tunnel) |
| `onionbit-db` (SQLite) | Faible | Moyenne | Reproduire le schéma logique de Pony ORM (torrents, canaux, votes, réglages) et ses migrations |
| `onionbit-network-policy` | Moyenne | Moyenne | Garde-fous de sécurité, logique métier propre mais critique |
| `onionbit-api` (REST + SSE) | Moyenne | Moyenne-Élevée | Beaucoup d'endpoints à couvrir pour la parité (11 fichiers côté Python) ; travail volumineux mais peu risqué techniquement |
| `onionbit-core` (orchestration) | Moyenne | Moyenne | Logique métier de `content_discovery`/`torrent_checker`/`rss`/`watch_folder` à reproduire |
| `onionbit-ipv8` (overlay) | **Élevé** | **Élevée** | Protocole propriétaire, aucune implémentation Rust de référence, NAT traversal, gestion du churn |
| `onionbit-tunnel` (TunnelCommunity) | **Élevé** | **Élevée** | Construction de circuits, crypto par saut, hidden seeding, résistance Sybil — dépend d'`onionbit-ipv8` fiable |
| Portage multiplateforme (build) | Moyenne | Moyenne | `librqbit`/Tokio sont déjà multiplateformes ; le vrai risque est l'exécution en arrière-plan sur mobile (limitations OS) et le modèle "Web" (cf. §7) |

## 6. Estimation de charge (ordre de grandeur, à titre indicatif)

Sans engagement de délai ferme (cf. règle sur les estimations de temps),
voici un ordre de grandeur du **volume de travail relatif** entre phases,
pour aider à prioriser — pas une promesse de durée :

- Fondations + moteur BitTorrent (étapes 1-8) : volume **modéré**, risque
  technique faible grâce à `librqbit`.
- API REST/DB/orchestration à parité fonctionnelle (étapes 4-8) : volume
  **important** (beaucoup d'endpoints/règles métier à couvrir) mais risque
  technique faible.
- IPv8 + TunnelCommunity (étapes 9-13) : volume **très important** et
  risque technique **élevé** — c'est la phase qui déterminera le plus la
  durée réelle du projet. Un développement incrémental avec des jalons
  d'interopérabilité contre de vrais pairs IPv8 (le réseau Tribler existant)
  est recommandé pour détecter les écarts de protocole le plus tôt possible.
- Packaging multiplateforme (étapes 17-19) : volume modéré, mais à ne pas
  sous-estimer pour Android/iOS (contraintes d'exécution en arrière-plan).

## 7. Risques et points d'attention

1. **IPv8/TunnelCommunity** : seul composant sans équivalent Rust mûr.
   Recommandation : commencer par un sous-ensemble minimal (discovery +
   une community triviale) et valider l'interopérabilité avec un noeud
   Tribler Python réel avant d'attaquer `TunnelCommunity`.
2. **Licence GPL-3.0** : si le code Python Tribler est consulté pour
   porter son comportement (ce qui est l'approche retenue), l'œuvre
   résultante doit rester GPL-3.0. Ne pas copier de code source verbatim ;
   porter la logique/le protocole. À faire valider par un avis juridique
   si une licence différente est envisagée un jour.
3. **Cible "Web"** : un navigateur ne peut pas ouvrir de sockets TCP/UDP
   bruts pour BitTorrent/DHT/IPv8. Le build Flutter Web devra donc parler,
   comme le fait déjà l'UI React actuelle de Tribler, à une instance du
   daemon Rust tournant sur la même machine ou un serveur distant, via
   HTTP/SSE — jamais un daemon "dans le navigateur". Hypothèse à
   documenter clairement dans le plan UI quand il sera rédigé.
4. **Android/iOS** : l'exécution prolongée en arrière-plan d'un daemon
   P2P (DHT, tunnels) est contrainte par les OS mobiles (App Nap, Doze,
   restrictions de service en arrière-plan). À traiter comme un sujet
   d'architecture dédié avant le portage mobile (étape 18), pas
   improvisé à la fin.
5. **Dérive de dépendance** : `librqbit` évolue vite (releases fréquentes,
   parfois beta) — figer une version testée dans `Cargo.lock` et revalider
   à chaque mise à jour volontaire.

## 8. Critères de "backend terminé à 100 %"

Avant de démarrer l'UI Flutter (cf. règle critique n°1 d'`AGENTS.md`),
chaque fonctionnalité de `docs/plans/roadmap.md` doit être :

- implémentée dans le crate approprié ;
- couverte par des tests automatisés (unitaires + intégration quand
  pertinent) qui passent via `scripts/verify_all.ps1` ;
- validée manuellement au moins une fois contre un scénario réel
  (téléchargement réel, circuit anonyme réel contre le réseau Tribler
  existant quand applicable) ;
- documentée dans `docs/reference_tribler/` (correspondance avec le
  comportement Python d'origine) si elle porte un comportement existant.
