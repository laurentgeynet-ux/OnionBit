# Architecture — Tribler-Rust-Torrent

## 1. Principes (clean architecture)

Le workspace suit une architecture en couches avec inversion de
dépendance : le domaine (`tribler-core`) ne dépend d'aucune
infrastructure concrète, il définit des **traits** (ports) que les
crates d'infrastructure implémentent (adaptateurs).

```
                        ┌───────────────────────────┐
                        │   Clients (hors backend)   │
                        │  tribler-cli | future UI   │
                        └─────────────┬─────────────┘
                                      │ HTTP / SSE
                        ┌─────────────▼─────────────┐
                        │        tribler-api         │  Interface (adaptateur entrant)
                        └─────────────┬─────────────┘
                                      │ appels de traits
                        ┌─────────────▼─────────────┐
                        │       tribler-core          │  Domaine + orchestration
                        │  Session / Notifier /       │  (ne dépend d'aucune I/O concrète)
                        │  règles métier              │
                        └──┬─────────┬─────────┬─────┘
                           │         │         │
              traits implémentés par les adaptateurs sortants :
                           │         │         │
          ┌────────────────┘   ┌─────┘   ┌─────┴───────────┐
          ▼                    ▼         ▼                 ▼
  tribler-bittorrent   tribler-tunnel  tribler-db   tribler-network-policy
  (via librqbit)        (via tribler-ipv8)  (rusqlite)   (garde-fous)
          │                    │
          ▼                    ▼
     [réseau BitTorrent]  tribler-ipv8 → [réseau overlay IPv8]

  Support transverse : tribler-format (formats), tribler-crypto (primitives),
  tribler-test-support (dev-dependency uniquement)
```

Règles de dépendance :

- `tribler-core` ne dépend **jamais** de `tribler-api`, `tribler-bittorrent`,
  `tribler-ipv8`, `tribler-tunnel` ou `tribler-db` concrètement — il expose
  des traits, ces crates les implémentent.
- `tribler-api` ne dépend que de `tribler-core` (jamais directement de
  `tribler-bittorrent`/`tribler-ipv8`/`tribler-db`).
- `tribler-daemon` est le seul crate autorisé à connaître **tout le monde**
  (c'est le point de câblage/composition racine, "main composition root").
- `tribler-tunnel` dépend de `tribler-ipv8` (et non l'inverse).
- `tribler-bittorrent` peut optionnellement utiliser le proxy SOCKS5 exposé
  par `tribler-tunnel` pour router du trafic, mais ne dépend pas de son
  code interne (frontière = SOCKS5 standard).

## 2. Couches et responsabilités

| Couche | Crate(s) | Analogue Python |
| :--- | :--- | :--- |
| Interface (entrée) | `tribler-api`, `tribler-cli` | `tribler.core.restapi` |
| Domaine / orchestration | `tribler-core` | `tribler.core.session`, `components.py`, `notifier.py`, `content_discovery/`, `torrent_checker/`, `rss/`, `watch_folder/` |
| Infrastructure — BitTorrent | `tribler-bittorrent` (sur `librqbit`) | `tribler.core.libtorrent` |
| Infrastructure — overlay anonyme | `tribler-ipv8`, `tribler-tunnel` | `pyipv8`, `tribler.core.tunnel` |
| Infrastructure — persistance | `tribler-db` | `tribler.core.database` |
| Infrastructure — sécurité réseau | `tribler-network-policy` | `tribler.core.socks5` (partiellement) + règles ajoutées pour la sécurité |
| Support transverse | `tribler-format`, `tribler-crypto`, `tribler-test-support` | utilitaires épars côté Python |
| Composition racine | `tribler-daemon` | point d'entrée `run.py`/`start_tribler.py` |

## 3. Flux d'événements

`tribler-core::Notifier` est le bus d'événements interne (équivalent du
`notifier.py` Python). Toute I/O qui produit un événement notable
(progression de téléchargement, changement d'état de circuit, nouveau
pair IPv8...) publie sur ce bus. `tribler-api` s'y abonne pour pousser
les événements en SSE (`text/event-stream`), exactement comme l'API
Python actuelle.

## 4. Pourquoi ne pas réécrire le moteur BitTorrent (ADR-0001)

Voir `docs/architecture/decisions/0001-moteur-bittorrent-rust-pur.md`.
Résumé : `librqbit` couvre déjà bencode/peer-wire/DHT/uTP/trackers en Rust
pur, activement maintenu, licence Apache-2.0 compatible GPL-3.0. Réécrire
ce protocole depuis zéro n'apporterait aucune valeur et retarderait la
partie réellement différenciante du projet (IPv8/anonymat).

## 5. Pourquoi IPv8 est le composant à plus haut risque (ADR-0002)

Voir `docs/architecture/decisions/0002-perimetre-ipv8-v1.md`. Résumé :
`ipv8-rust-tunnels` ne couvre que le plan de données ; le protocole
complet (discovery, communities, DHT overlay, crypto, construction de
circuits) doit être porté depuis `pyipv8`/`tribler.core.tunnel` sans
équivalent Rust de référence.

## 6. Multiplateforme

- Desktop (Windows x64/arm64, Linux, macOS) : `tribler-daemon` tourne en
  processus natif, `tribler-api` écoute sur `127.0.0.1`.
- Android/iOS : `tribler-daemon` (ou une variante allégée) tourne comme
  service applicatif ; sujet dédié à traiter avant l'étape 14 (contraintes
  d'exécution en arrière-plan, cf. `plan_faisabilite.md` §7).
- Web : pas de daemon dans le navigateur ; le futur frontend Flutter Web
  se connecte en HTTP/SSE à une instance `tribler-daemon` locale ou
  distante, comme le fait déjà l'UI React actuelle de Tribler.
