# Architecture — OnionBit

> OnionBit est un écosystème pair-à-pair anonyme et résistant à la censure
> — une toile onion sans serveur qui porte ses propres services : partage
> de fichiers, messagerie et identité portable. Né comme portage Rust de
> Tribler, il est devenu écosystème autonome — compatible avec Tribler,
> défini par ses extensions. Définition canonique et vocabulaire normé :
> [ADR-0020](decisions/0020-definition-ecosysteme-p2p-anonyme.md). La
> colonne « Analogue Python » ci-dessous décrit la filiation et la
> compatibilité protocole, pas la définition du produit.

## 1. Principes (clean architecture)

Le workspace suit une architecture en couches avec inversion de
dépendance : le domaine (`onionbit-core`) ne dépend d'aucune
infrastructure concrète, il définit des **traits** (ports) que les
crates d'infrastructure implémentent (adaptateurs).

```
                        ┌───────────────────────────┐
                        │   Clients (hors backend)   │
                        │  onionbit-cli | future UI   │
                        └─────────────┬─────────────┘
                                      │ HTTP / SSE
                        ┌─────────────▼─────────────┐
                        │        onionbit-api         │  Interface (adaptateur entrant)
                        └─────────────┬─────────────┘
                                      │ appels de traits
                        ┌─────────────▼─────────────┐
                        │       onionbit-core          │  Domaine + orchestration
                        │  Session / Notifier /       │  (ne dépend d'aucune I/O concrète)
                        │  règles métier              │
                        └──┬─────────┬─────────┬─────┘
                           │         │         │
              traits implémentés par les adaptateurs sortants :
                           │         │         │
          ┌────────────────┘   ┌─────┘   ┌─────┴───────────┐
          ▼                    ▼         ▼                 ▼
  onionbit-bittorrent   onionbit-tunnel  onionbit-db   onionbit-network-policy
  (via librqbit)        (via onionbit-ipv8)  (rusqlite)   (garde-fous)
          │                    │
          ▼                    ▼
     [réseau BitTorrent]  onionbit-ipv8 → [réseau overlay IPv8]

  Support transverse : onionbit-format (formats), onionbit-crypto (primitives),
  onionbit-test-support (dev-dependency uniquement)
```

Règles de dépendance :

- `onionbit-core` ne dépend **jamais** de `onionbit-api`, `onionbit-bittorrent`,
  `onionbit-ipv8`, `onionbit-tunnel` ou `onionbit-db` concrètement — il expose
  des traits, ces crates les implémentent.
- `onionbit-api` consomme `onionbit-core` pour la logique métier ; il peut
  importer les **types/DTO** des crates d'infrastructure
  (`onionbit-bittorrent`, `onionbit-format`, `onionbit-ipv8`,
  `onionbit-tunnel`, `onionbit-db`) pour la sérialisation — jamais pour
  y déléguer de la logique de domaine.
- `onionbit-daemon` est le seul crate autorisé à connaître **tout le monde**
  (c'est le point de câblage/composition racine, "main composition root").
- `onionbit-tunnel` dépend de `onionbit-ipv8` (et non l'inverse).
- `onionbit-bittorrent` peut optionnellement utiliser le proxy SOCKS5 exposé
  par `onionbit-tunnel` pour router du trafic, mais ne dépend pas de son
  code interne (frontière = SOCKS5 standard).

## 2. Couches et responsabilités

| Couche | Crate(s) | Analogue Python |
| :--- | :--- | :--- |
| Interface (entrée) | `onionbit-api`, `onionbit-cli` | `tribler.core.restapi` |
| Domaine / orchestration | `onionbit-core` | `tribler.core.session`, `components.py`, `notifier.py`, `content_discovery/`, `torrent_checker/`, `rss/`, `watch_folder/` |
| Infrastructure — BitTorrent | `onionbit-bittorrent` (sur `librqbit`) | `tribler.core.libtorrent` |
| Infrastructure — overlay anonyme | `onionbit-ipv8`, `onionbit-tunnel` | `pyipv8`, `tribler.core.tunnel` |
| Infrastructure — persistance | `onionbit-db` | `tribler.core.database` |
| Infrastructure — sécurité réseau | `onionbit-network-policy` | `tribler.core.socks5` (partiellement) + règles ajoutées pour la sécurité |
| Support transverse | `onionbit-format`, `onionbit-crypto`, `onionbit-test-support` | utilitaires épars côté Python |
| Composition racine | `onionbit-daemon` | point d'entrée `run.py`/`start_tribler.py` |

## 3. Flux d'événements

`onionbit-core::Notifier` est le bus d'événements interne (équivalent du
`notifier.py` Python). Toute I/O qui produit un événement notable
(progression de téléchargement, changement d'état de circuit, nouveau
pair IPv8...) publie sur ce bus. `onionbit-api` s'y abonne pour pousser
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

- Desktop (Windows x64/arm64, Linux, macOS) : `onionbit-daemon` tourne en
  processus natif, `onionbit-api` écoute sur `127.0.0.1`.
- Android/iOS : `onionbit-daemon` (ou une variante allégée) tourne comme
  service applicatif ; sujet dédié à traiter avant l'étape 14 (contraintes
  d'exécution en arrière-plan, cf. `plan_faisabilite.md` §7).
- Web : pas de daemon dans le navigateur ; le futur frontend Flutter Web
  se connecte en HTTP/SSE à une instance `onionbit-daemon` locale ou
  distante, comme le fait déjà l'UI React actuelle de Tribler.
