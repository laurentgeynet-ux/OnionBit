# Écarts de fidélité protocole assumés

Divergences volontaires par rapport aux constantes/comportements filaires
de Tribler (hors ADR — cf. `docs/architecture/decisions/`). Chaque écart
indique la valeur Python de référence, la valeur Rust et le motif.

## `ContentDiscoverySettings.max_query_peers` : 60 (Python : 20)

- **Python** : `random.sample(peers, min(len(peers), 20))` dans
  `send_search_request` — 20 pairs maximum par `remote_select` de
  recherche.
- **Rust** : `max_query_peers = 60`
  (`crates/onionbit-ipv8/src/content_discovery.rs`), même tirage sans
  remise borné par `len(peers)`.
- **Motif** : l'overlay content-discovery accumule typiquement >200
  pairs connus (la cible `RandomWalk(20)` est un plancher, pas un
  plafond). À 20, une requête ne touchait que ~10 % des pairs
  disponibles ; à 60 on triple le rappel par requête sans changer la
  sémantique filaire (même `SelectPayload`, même `SelectResponse`).
- **Coût** : jusqu'à 3× plus de `remote_select` en vol par requête et
  de réponses entrantes ; `select_ttl` (10 s) et `packets_limit` (10)
  inchangés bornent l'effort par pair.

## `libtorrent/allow_mmap` : défaut `false` (Tribler : `true`)

- **Python** : `libtorrent/allow_mmap = true` — backend de stockage
  mmap de libtorrent sur la session en clair.
- **Rust** : `allow_mmap` vaut `false` par défaut
  (`crates/onionbit-core/src/daemon_config.rs`,
  `crates/onionbit-bittorrent/src/config.rs`) → `FilesystemStorage`
  rqbit (écritures `WriteFile`/`pwritev` positionnées, synchrones par
  bloc de 16 Kio). La clé reste câblée : `true` réinstalle
  `MmapFilesystemStorageFactory` (parité Tribler).
- **Motif** : alignement sur le défaut **upstream rqbit** (mmap n'y
  est qu'un backend d'exemple opt-in). Sous Windows, les pages
  modifiées d'un mmap ne passent pas par le throttling du cache
  manager : un téléchargement plus rapide que le disque accumulait
  les pages sales sans borne (~5 Go observés sur 16 Go avant
  flush/arrêt — diagnostic
  `docs/diagnostics/memoire_charge_reelle.md`, 2026-10-09).
- **Coût** : un syscall positionné par bloc écrit au lieu d'un
  `memcpy` dans le mapping ; la relecture `check_piece` et le service
  d'upload passent par le cache NTFS au lieu de pages déjà mappées.
