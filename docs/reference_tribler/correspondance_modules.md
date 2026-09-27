# Correspondance modules Python Tribler ↔ crates Rust

Table vivante, à compléter au fil des étapes de `docs/plans/roadmap.md`
avec les écarts de comportement constatés lors du portage.

| Module Python (`D:\Projet\Tribler_sources\tribler\src\tribler\core`) | Crate Rust | Étape roadmap | Écarts constatés |
| :--- | :--- | :--- | :--- |
| `session.py`, `components.py`, `notifier.py` | `tribler-core` | 5 | — |
| `libtorrent/` | `tribler-bittorrent` (+ dépendance `librqbit`) | 3 | Moteur sous-jacent différent (Rust `librqbit` vs C++ `libtorrent-rasterbar`) — parité de comportement à vérifier BEP par BEP |
| `database/` | `tribler-db` | 4 | Pony ORM → `rusqlite` : schéma à revalider, pas de portage 1:1 du mapping objet |
| `restapi/` | `tribler-api` | 6, 15 | Voir `api_rest_mapping.md` |
| `tunnel/` | `tribler-tunnel` | 12 | — |
| `content_discovery/` | `tribler-core` | 14 | — |
| `torrent_checker/` | `tribler-core` | 14 | — |
| `socks5/` | `tribler-tunnel` + `tribler-network-policy` | 12-13 | — |
| `rss/` | `tribler-core` | 14 | — |
| `watch_folder/` | `tribler-core` | 14 | — |
| `versioning/` | `tribler-daemon` (config/migrations) | — | — |
| `pyipv8/ipv8/` | `tribler-ipv8` | 9-11 | Référence de vérité : `D:\Projet\Tribler_sources\tribler\pyipv8` |

## `api_rest_mapping.md`

Fichier à créer à l'étape 6 (premiers endpoints) et complété à l'étape 15
(parité complète) : table `méthode + chemin Python` → `méthode + chemin
Rust` → `statut` (identique / adapté / non porté + justification).
