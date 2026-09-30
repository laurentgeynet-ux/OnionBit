# Plan — Enrichissement de la sidebar (UI Flutter)

Date : 2026-10-02. Demande utilisateur : rendre le menu latéral plus
moderne (même démarche que `app_downloads_enrichissement.md`,
`app_search_enrichissement.md`, `app_settings_enrichissement.md`).

L'existant (`app/lib/core/layout/app_sidebar.dart`) : `ListTile` plats,
en-tête logo texte, bouton « Ajouter », sous-filtres avec compteurs en
texte brut. Le catalogue `kNavCatalog` reste la source unique des
destinations (ne pas dupliquer).

Chaque étape = implémentation + `flutter analyze`/tests + cochage ici +
`docs/CHANGELOG.md` + commit dédié. `vendor/` hors périmètre.

## Étape 1 — Style moderne : items « pill » + groupes + badges [x] (2026-10-02)

- Remplacer les `ListTile` par des items maison : surbrillance pilule
  arrondie (`BorderRadius` + `InkWell`), marge latérale, icône+label,
  compteurs en badge `surfaceContainerHighest`.
- En-têtes de groupe (« Bibliothèque », « Système ») en labelSmall
  outline.
- Badge rouge compteur `STOPPED_ON_ERROR` sur l'entrée Téléchargements.

## Étape 2 — Bloc débit global dans l'en-tête

- Sous le bouton « Ajouter » : ligne ↓/↑ temps réel
  (`totalSpeedsProvider` déjà existant) en `bodySmall`, avec icônes.

## Étape 3 — Logo OnionBit (SVG)

- `flutter_svg` + asset `branding/logo-horizontal.svg` (copie dans
  `app/assets/branding/`) dans l'en-tête à la place du shield+texte.
- Fallback `Icons.shield_outlined` si l'asset n'est pas trouvé.

## Étape 4 — Filtres repliables

- Le groupe de sous-filtres devient repliable (chevron sur l'entrée
  parente Téléchargements) ; état conservé dans un provider session.
- La navigation vers `/downloads` reste au clic sur le label ; le
  chevron seul replie/déplie.

## Étape 5 — Mode rail rétractable

- Bouton en bas de sidebar : bascule largeur 216 px ↔ rail icônes
  (~72 px, labels masqués, `Tooltip` sur les entrées).
- État dans un provider session (pas de persistance disque).

## Étape 6 — Pied de sidebar : état daemon

- En bas : pastille connexion (`sseConnectedProvider`) + « Daemon » +
  version (`versionsProvider` settings) ; lecture seule.
