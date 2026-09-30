# Plan — Enrichissement de l'onglet Réglages (UI Flutter)

Date : 2026-09-30. Demande utilisateur : enrichir l'onglet « Réglages »
comme les onglets Téléchargements (`app_downloads_enrichissement.md`)
et Rechercher (`app_search_enrichissement.md`).

L'existant : 11 sections `SettingsSection` (scaffold Card + builder sur
`GET /api/settings`), sauvegarde par section via `applySettingsPatch`
(POST merge récursif), commutateurs à sauvegarde immédiate
(`SettingsSwitch`). Sections à sauvegarde différée : Bandwidth, Queue,
Downloads, Seeding, Automation — les autres appliquent à chaud.

Chaque étape = implémentation + `flutter analyze`/tests + cochage ici +
`docs/CHANGELOG.md` + commit dédié. `vendor/` hors périmètre.

## Étape 1 — Table des matières + recherche dans les réglages [x] (2026-09-30)

- Catalogue des sections (`_kSections` : titre, icône, mots-clés,
  widget) au lieu de la liste littérale — un seul endroit de vérité.
- Rail de chips en haut → `Scrollable.ensureVisible` (chaque section
  enveloppée dans un `KeyedSubtree`).
- Champ « Filtrer les réglages » : filtre le catalogue sur titre +
  mots-clés + chemins de clés ; les sections masquées ne sont pas
  construites.

## Étape 2 — Indicateur « modifié » + « Enregistrer tout » [x] (2026-10-02)

- `settingsDirtyProvider` (Set d'ids de sections) + bus
  `settingsSaveBusProvider` (Map id → callback `_save`).
- Les sections à sauvegarde différée déclarent dirty sur changement de
  champ (listeners sur les controllers) et s'enregistrent au bus.
- Puce « modifié » dans le titre de la section (`SettingsSection`
  accepte `dirtyId` et l'affiche depuis le provider) + bandeau collant
  « N section(s) modifiée(s) · Enregistrer tout / Tout annuler ».

## Étape 3 — Tooltips « clé configuration.json »

- Widget `KeyInfoIcon(path, description)` — icône `i` affichant le
  chemin de la clé (`libtorrent/max_download_rate`) + texte de l'aide
  existante.
- Déployé sur les champs principaux : Bandwidth, Queue, Downloads,
  Seeding, Anonymity, Network, Automation.

## Étape 4 — « Rétablir les défauts » par section

- `kSettingsDefaults` (map des défauts documentés —
  `api_endpoints_complet.md` / `configuration_cablage.md`).
- Bouton « Défauts » par section concernée → patch des défauts via
  `applySettingsPatch`, resync des controllers.

## Étape 5 — Section « Avancé » : arbre configuration + export/import

- `AdvancedSection` : arbre récursif de `GET /api/settings` (feuilles
  éditables via dialogue — bool/int/string selon le type) →
  `applySettingsPatch` partiel.
- Export : « Copier la configuration » (JSON complet dans le
  presse-papier). Import : « Coller » → aperçu → `POST /api/settings`
  (merge récursif côté backend).

## Étape 6 — Bande passante : sliders + presets

- `Slider` logarithmique par direction (0 = illimité ↔ 10 Mo/s) en plus
  du champ numérique, synchronisés ; chips presets
  (illimité, 1/5/10 Mo/s) cohérents avec le menu contextuel des
  téléchargements.

## Étape 7 — Réseau : état réel en lecture seule

- Carte d'état dans `NetworkSection` : port d'écoute, DHT, UPnP/NAT-PMP,
  proxy actif — valeurs lues depuis l'arbre de réglages + état runtime
  si exposé (`/api/statistics/tribler` ou ipv8 overlays) ; lecture
  seule, distinguée des réglages éditables.
