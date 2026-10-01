# ADR-0005 — Code et documentation en français

Statut : Acceptée (2026-09-27).

## Contexte

L'utilisateur pilote plusieurs projets Rust similaires (ex. eMule-Rust)
avec code et documentation en français. Le dépôt source amont Tribler
et son écosystème (issues, BEPs, spécifications IPv8) sont en anglais.

## Décision

Code, commentaires, documentation et messages de commit de ce projet
sont rédigés en français, cohérent avec les autres projets de
l'utilisateur. Les identifiants techniques calqués sur des noms officiels
(BEP, opcodes IPv8, noms de champs JSON de l'API REST Tribler existante)
restent dans leur forme originale (anglaise) pour rester traçables face
à la référence de vérité (`<Tribler sources checkout> (env `TRIBLER_SRC`)`).

## Conséquences

- Les docstrings Rust (`//!`, `///`) sont en français.
- Les noms de types/fonctions/variables suivent les conventions Rust
  standard (anglais) uniquement quand ils reprennent un terme officiel du
  protocole (ex. `CommunityId`, `HopCount`) ; sinon, privilégier des noms
  français explicites pour la logique métier propre au projet.
