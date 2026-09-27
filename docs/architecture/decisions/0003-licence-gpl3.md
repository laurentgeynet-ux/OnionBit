# ADR-0003 — Licence GPL-3.0-or-later

Statut : Acceptée (2026-09-27).

## Contexte

Tribler est publié sous licence **GPL-3.0**. Ce projet porte
l'architecture et le comportement de Tribler (protocole IPv8, logique de
session, schéma de métadonnées, contrat d'API REST) vers Rust, en
s'appuyant activement sur les sources officielles comme référence de
vérité (`D:\Projet\Tribler_sources\tribler`). Il ne s'agit pas d'une
réimplémentation "clean-room" (à la différence, par exemple, du projet
eMule-Rust de l'utilisateur qui reconstruit un protocole legacy sans
réutiliser de code sous licence copyleft comme référence de comportement
métier).

## Décision

Le projet est publié sous licence **GPL-3.0-or-later** (fichier
`LICENSE` à la racine, texte officiel FSF).

## Justification

- Porter la logique/le comportement d'un travail GPL-3.0, même sans
  copier le texte source, crée une œuvre dérivée dont la distribution
  doit respecter les termes de la GPL-3.0 (copyleft "fort").
- Une dépendance Apache-2.0 (`librqbit`) est compatible et peut être
  incluse dans un projet GPL-3.0 sans reciprocité de licence sur cette
  dépendance elle-même.

## Conséquences

- Toute dépendance ajoutée doit être vérifiée compatible GPL-3.0 avant
  ajout (cf. `AGENTS.md`, règle critique n°5).
- Si l'utilisateur souhaite un jour une licence différente (ex. pour un
  usage commercial fermé), cela nécessite un avis juridique dédié — ce
  n'est pas une simple modification de fichier `LICENSE`.
- Ne jamais copier de code source Python Tribler verbatim dans ce dépôt ;
  porter la logique/le protocole en réécrivant depuis la compréhension du
  comportement, pas en traduisant ligne à ligne.
