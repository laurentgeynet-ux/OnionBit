# Correspondance modules Python Tribler ↔ crates Rust

Table vivante, à compléter au fil des étapes de `docs/plans/roadmap.md`
avec les écarts de comportement constatés lors du portage.

| Module Python (`<Tribler sources checkout> (env `TRIBLER_SRC`)\src\tribler\core`) | Crate Rust | Étape roadmap | Écarts constatés |
| :--- | :--- | :--- | :--- |
| `session.py`, `components.py`, `notifier.py` | `onionbit-core` | 5 | — |
| `libtorrent/` | `onionbit-bittorrent` (+ dépendance `librqbit`) | 3 | Moteur sous-jacent différent (Rust `librqbit` vs C++ `libtorrent-rasterbar`) — parité de comportement à vérifier BEP par BEP |
| `database/` | `onionbit-db` | 4 | Pony ORM → `rusqlite` : schéma à revalider, pas de portage 1:1 du mapping objet |
| `restapi/` | `onionbit-api` | 6, 15 | Voir `api_rest_mapping.md` |
| `tunnel/` | `onionbit-tunnel` | 12 | — |
| `content_discovery/` | `onionbit-core` | 14 | — |
| `torrent_checker/` | `onionbit-core` | 14 | — |
| `socks5/` | `onionbit-tunnel` + `onionbit-network-policy` | 12-13 | — |
| `rss/` | `onionbit-core` | 14 | — |
| `watch_folder/` | `onionbit-core` | 14 | — |
| `versioning/` | `onionbit-daemon` (config/migrations) | — | — |
| `pyipv8/ipv8/` | `onionbit-ipv8` | 9-11 | Référence de vérité : `<Tribler sources checkout> (env `TRIBLER_SRC`)\pyipv8` |

## `api_rest_mapping.md`

Fichier créé à l'étape 6 (premiers endpoints) et à compléter à l'étape 15
(parité complète) : table `méthode + chemin Python` → `méthode + chemin
Rust` → `statut` (identique / adapté / non porté + justification).
