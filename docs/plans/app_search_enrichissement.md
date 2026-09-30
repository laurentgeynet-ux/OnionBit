# Plan — Enrichissement de l'onglet Rechercher (UI Flutter)

Date : 2026-09-30. Demande utilisateur : enrichir l'onglet « Rechercher »
comme l'onglet « Téléchargements » (`app_downloads_enrichissement.md`).
L'existant (`search_page.dart`) : ListTiles, tri via `PopupMenuButton`,
fusion local+distant dédupliquée par infohash, spinner de recherche
distante avec nombre de pairs.

Chaque étape = implémentation + `flutter analyze`/tests + cochage ici +
`docs/CHANGELOG.md` + commit dédié. Les modifications `vendor/librqbit`
en cours restent hors périmètre.

Constantes backend vérifiées (2026-09-30) :

- `GET /api/metadata/search/local` renvoie `updated` (`torrent_date`) —
  non mappé côté `TorrentResult`.
- `GET /api/metadata/torrents/{infohash}/health` — sonde de santé à la
  demande.
- `PUT /api/downloads` accepte `anon_hops` (0-3) + `safe_seeding`.

## Étape 1 — Table desktop triable + date + santé [x] (2026-09-30)

- `TorrentResult.date` mappé depuis `updated` (epoch ou ISO — vérifier
  le format réel à l'implémentation).
- Table desktop avec en-têtes triables : Nom, Taille, Seeds, Leechers,
  Date, Source ; comparateurs partagés.
- Pastille de santé réutilisant le pattern `_HealthDot` (seeds>0 =
  vert, leechers seuls = orange, rien = rouge/gris).
- Compact : ListTiles conservés.

## Étape 2 — Menu contextuel + ajout anonyme [x] (2026-09-30)

- Clic droit / appui long : Ajouter, sous-menu « Ajouter en anonyme »
  (0-3 sauts — `add(uri:, anonHops: h, safeSeeding: true)`), Copier le
  magnet, Copier l'info-hash.

## Étape 3 — Badge « déjà téléchargé » [x] (2026-09-30)

- Croisement `infohash` × `downloadsProvider` : chip « En cours » sur
  les lignes connues, bouton Ajouter désactivé (tooltip).

## Étape 4 — Sélection multiple + ajout en lot [x] (2026-09-30)

- Cases + provider de sélection (même pattern que downloads) ; bouton
  « Ajouter la sélection » dans la barre d'outils quand sélection ≠ ∅.

## Étape 5 — Surlignage + filtres [x] (2026-09-30)

- Termes de la requête surlignés dans les noms (`Text.rich` segments).
- Chips de filtre : source (tous/local/réseau), seeds ≥ 10.

## Étape 6 — Recherche distante : Stop + compteur [x] (2026-09-30)

- Bouton Stop (coupe la fenêtre de collecte `_collectRemote`), compteur
  « N nouveaux résultats », timestamp de fin affiché.

## Étape 7 — Historique des recherches récentes [x] (2026-09-30)

- `searchHistoryProvider` (session, 10 max, LRU) alimenté à chaque
  requête non vide ; chips « Récents » quand la requête est vide, sous
  le titre « Populaires ».

## Étape 8 — Sonde de santé à la demande

- Action « Rafraîchir la santé » dans le menu contextuel (et icône en
  fin de ligne pour la sélection courante) →
  `/metadata/torrents/{ih}/health` → mise à jour de seeds/leechers de
  la ligne, sans recharger la liste.
