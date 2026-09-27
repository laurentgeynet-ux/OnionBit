# ADR-0004 — Structure en workspace Cargo multi-crates, inspirée d'eMule-Rust

Statut : Acceptée (2026-09-27).

## Contexte

L'utilisateur maintient déjà un projet Rust similaire par nature —
**eMule-Rust** (`C:\Emule-Sion-UI-UX`) — un portage Rust d'un protocole
P2P legacy (eD2K/Kad) avec un daemon Tokio, un plan de contrôle
JSON-RPC/WebSocket, une CLI, et des règles d'ingénierie strictes
documentées dans son `AGENTS.md` (pas de valeurs en dur, un composant par
fichier, anti-duplication, `thiserror`/`tracing`, `-D warnings`).

## Décision

Reprendre la même forme de structure pour Tribler-Rust-Torrent : un
workspace Cargo avec des crates `tribler-*` par responsabilité (cf.
`docs/architecture/architecture.md`), les mêmes règles d'ingénierie
(adaptées dans `AGENTS.md`), et un script `scripts/verify_all.ps1`
équivalent.

## Justification

- Cohérence d'écosystème pour l'utilisateur, qui connaît déjà ces
  conventions et outils (moins de coût cognitif).
- Séparation claire domaine/infrastructure dès le départ, nécessaire
  pour respecter la contrainte "backend à 100 % avant l'UI" sans
  accumuler de dette d'architecture.

## Conséquences

- 12 crates au démarrage (`tribler-format`, `tribler-crypto`,
  `tribler-bittorrent`, `tribler-ipv8`, `tribler-tunnel`, `tribler-core`,
  `tribler-db`, `tribler-network-policy`, `tribler-api`, `tribler-cli`,
  `tribler-daemon`, `tribler-test-support`), cf.
  `docs/architecture/architecture.md` pour le détail des responsabilités.
- Contrairement à eMule-Rust, pas de crate `tribler-mcp` au démarrage
  (serveur MCP pour agents IA) : possible extension future si souhaité,
  non bloquante pour le backend BitTorrent/IPv8.
