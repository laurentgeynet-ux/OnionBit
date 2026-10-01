# ADR-0001 — Moteur BitTorrent : Rust pur via `librqbit`, pas de FFI vers libtorrent-rasterbar

Statut : Acceptée (2026-09-27).

## Contexte

Tribler Python s'appuie sur `libtorrent` (bindings Python du C++
`libtorrent-rasterbar`) pour le moteur BitTorrent. Deux options
existaient pour le portage Rust :

1. Lier `libtorrent-rasterbar` en FFI depuis Rust.
2. Utiliser/écrire un moteur BitTorrent Rust pur.

## Décision

Option 2, en s'appuyant sur le crate **`librqbit`** (Apache-2.0) et ses
sous-crates (`librqbit-core`, `librqbit-bencode`, `dht`,
`peer_binary_protocol`, `tracker_comms`, `upnp`) plutôt qu'une
réécriture complète depuis zéro.

## Justification

- Cible multiplateforme explicite (Windows x64/arm64, Linux, macOS,
  **Android, iOS, Web**) : un FFI C++ complique fortement les builds
  mobiles/WASM et la compilation croisée.
- `librqbit` est actif, mûr (~1600★, 61 releases), couvre BEP 3/5/9/10,
  uTP, tracker HTTP/UDP — exactement le socle nécessaire.
- Licence Apache-2.0, compatible avec la licence GPL-3.0-or-later du
  projet (dépendance permissive incluse dans un projet copyleft : OK).
- Concentre l'effort de développement réel sur ce qui n'a pas
  d'équivalent Rust : IPv8/TunnelCommunity (cf. ADR-0002).

## Conséquences

- `onionbit-bittorrent` est un crate d'adaptation (traduit l'API
  `librqbit::Session` vers les traits/domaine de `onionbit-core`), pas un
  moteur bas niveau.
- Dépendance externe dont il faut suivre les versions (parfois beta) —
  figer dans `Cargo.lock`, revalider à chaque mise à jour volontaire.
- Si `librqbit` s'avère insuffisant sur un point précis (ex. BEP manquant
  nécessaire à l'interopérabilité avec le réseau Tribler), évaluer une
  contribution amont avant d'envisager un fork ou une réécriture locale.
