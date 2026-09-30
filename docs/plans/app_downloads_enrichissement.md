# Plan — Enrichissement de l'onglet Téléchargements (UI Flutter)

Date : 2026-09-30. Demande utilisateur : rendre l'onglet « Téléchargements »
moins basique. L'existant (`downloads_page.dart`, `download_detail_panel.dart`)
couvre déjà : table + liste compacte, panneau de détail à onglets
(Détails/Fichiers/Trackers/Pairs), menu contextuel complet (file d'attente,
sauts anonymes, limites via dialogue, ratio, recheck, move_storage,
clone_public, copie magnet/info-hash, suppression).

Les étapes ci-dessous ajoutent la couche « client torrent moderne ».
Chaque étape = implémentation + `flutter analyze`/tests + mise à jour de ce
fichier (cochage) + `docs/CHANGELOG.md` + commit dédié. Les modifications
`vendor/librqbit` en cours sont hors périmètre (jamais commitées ici).

## Étape 1 — Tri par colonnes + sélection clavier/souris [x] (2026-09-30)

- En-têtes cliquables (Nom, Taille, Progression, ↓, ↑, ETA, Pairs, État) avec
  tri asc/desc et indicateur visuel.
- Sélection : clic simple = sélection unique ; Ctrl+clic = toggle ;
  Shift+clic = plage depuis l'ancre ; Ctrl+A = tout ; Échap = vider.
- `DownloadSortNotifier` (colonne + direction) persistant en session.

## Étape 2 — Lignes enrichies [x] (2026-09-30)

- % affiché au bout de la barre de progression (texte, pas seulement chip).
- Badges : erreur (icône + tooltip `error`), `isPrivate`, position de file
  (`queuePosition ≥ 0`), icône pause explicite.
- Colonnes supplémentaires : Ratio, Ajouté le.
- Pastille santé colorée dans la colonne Pairs (vert/orange/rouge selon
  `numSeeds`/`numConnectedPeers`).

## Étape 3 — Presets de limites de débit dans le menu contextuel [x] (2026-09-30)

- Sous-menu « Limites de débit » : upload et download, presets
  64 / 128 / 512 Kio/s / 1 Mio/s / illimité, coche sur la valeur courante,
  entrée « Personnalisé… » renvoyant au dialogue existant.

## Étape 4 — Sparkline de débit temps réel [x] (2026-09-30)

- Historique glissant (120 points, 1 échantillon/poll) des débits down/up du
  téléchargement sélectionné, affiché en haut de l'onglet Détails.
- `CustomPainter` maison (pas de dépendance chart pour un sparkline).

## Étape 5 — Raccourcis clavier [x] (2026-09-30)

- Sur la page : Espace = pause/reprendre la sélection, Suppr = supprimer
  (avec le dialogue existant), Ctrl+A, Échap, F2 = limites.
- `FocusableActionDetector`/`Shortcuts`/`Actions` Material.

## Étape 6 — Drag & drop [x] (2026-09-30)

- `DropTarget` (package `desktop_drop`, MIT — déjà standard pour Flutter
  desktop) : `.torrent` → `addTorrentBytes`, `magnet:` → `add(uri:)`.
- Surbrillance de la liste pendant le survol. Web : non applicable
  (DropTarget desktop only) — documenter l'écart.

## Étape 7 — Onglet Pairs enrichi [x] (2026-09-30)

- Table : adresse, client (`extended_version`), direction (entrant/sortant),
  débits ↓/↑, totaux échangés, transport (`connection_type`).
- Nécessite `?get_peers=1` — provider dédié avec poll tant que l'onglet est
  visible.

## Étape 8 — Notification de complétion

- Snackbar global sur l'événement SSE `torrent_finished` (avec nom du
  torrent), dans le shell — pas dans la page.
