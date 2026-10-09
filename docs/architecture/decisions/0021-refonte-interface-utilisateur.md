# ADR-0021 — Refonte de l'interface utilisateur : design system premium et adaptabilité multiplateforme

Statut : **Acceptée** (2026-10-09, après une revue de la proposition —
clarification macOS/iOS intégrée en §3/§4/§10/Limites). Exécution sur
le worktree dédié `adr21` (convention « Worktree dédié » d'ADR-0020
§7, déjà appliquée à ADR-0017/`adr17` et ADR-0020/`adr20`) ; Phase 13
ouverte dans `docs/plans/roadmap.md` (étapes 70–77) — implémentation
à venir, aucune étape encore livrée à ce jour. Périmètre strictement
`app/` : aucune évolution d'`onionbit-api` ou `onionbit-core` n'est
requise par cette décision — une unique dépendance croisée est
identifiée, isolée et différée à sa propre revue (étape 76/§9).

## Contexte

L'UI Flutter (`app/`, module `onionbit_ui`) existe depuis l'étape 20
(Phase 6, 2026-09-27) et a grandi au rythme des ADR livrés depuis :
internationalisation (ADR-0009), messagerie (ADR-0011, ADR-0019),
extensions de confiance (ADR-0015), identité portable (ADR-0016),
transport furtif (ADR-0017), zones de stockage (ADR-0018). Chaque
étape a ajouté ses écrans et ses réglages sans jamais revisiter
l'ensemble — la dette s'est accumulée par appositions successives,
jamais par une malfaçon ponctuelle :

- **Le design system n'a jamais été conçu pour OnionBit.**
  `core/theme/app_theme.dart` le documente lui-même : « Design system
  Material 3 de l'application — repris de l'app de référence
  (`C:\Emule-Sion-UI-UX\app`), recalé sur la palette de la marque
  OnionBit. » Le thème de départ (roadmap étape 20 : « thème eMule,
  seed `0xFF2F6FED` ») vient d'un projet tiers sans rapport avec
  l'identité d'écosystème anonyme/anti-censure actée par ADR-0020 ;
  seule la couleur de seed a changé depuis (`#6C2EA6`). Les seuls
  tokens formalisés sont `AppSpacing` (5 paliers) et `AppRadii`
  (3 paliers) — aucune échelle typographique, aucun token de motion,
  aucun système d'élévation au-delà des valeurs par défaut de
  `ThemeData`.
- **Aucune documentation vivante du design.** Le look-and-feel vit
  uniquement dans le code des widgets (`core/widgets/`,
  `features/*/presentation/widgets/`) ; rien n'énonce les règles
  (quand utiliser une `Card`, un `ActionChip`, quelle densité). Toute
  dérive visuelle entre écrans est donc invisible tant qu'elle n'est
  pas vue à l'œil — exactement la dynamique de « modifications
  manuelles » qui motive cet ADR.
- **Le responsive est déclaré mais sous-exploité.**
  `core/layout/breakpoints.dart` définit quatre paliers (`compact`
  < 600dp, `medium` 600–1024, `expanded` 1024–1600, `large` ≥ 1600)
  mais `AppShell` (`core/layout/app_shell.dart`) ne tranche qu'entre
  `compact` (barre de nav basse) et tout le reste (sidebar fixe
  identique de la tablette au moniteur ultra-wide). Aucun layout
  dédié tablette (liste+détail côte à côte), aucune troisième colonne
  desktop grand écran.
- **Le clavier existe par îlots, jamais généralisé.**
  `downloads_page.dart` câble déjà des `CallbackShortcuts`
  (Ctrl+A sélectionner tout, Échap désélectionner, Espace
  pause/reprise, Suppr retirer, F2 limites de débit) — mais
  uniquement en vues grille/tableau desktop ; rien en liste compacte,
  rien hors Téléchargements, aucun ordre de focus explicite, aucune
  recherche globale. C'est la preuve que le besoin est reconnu
  feature par feature sans jamais être traité comme un système.
- **La croissance organique des réglages.**
  `features/settings/presentation/pages/settings_page.dart` empile
  15 sections (apparence, téléchargements, stockage, bande passante,
  file d'attente, seeding, anonymat, extensions OnionBit, stealth,
  identité, réseau, automatisation, versions, connexion, daemon) dans
  une seule liste défilante avec puces d'ancrage et filtre texte —
  fonctionnel, mais une architecture d'information « tiroir à
  bric-à-brac », pas un parcours hiérarchisé.
- **Le multiplateforme annoncé n'est pas scaffoldé.** `app/README.md`
  affirme déjà : « Interface Flutter d'OnionBit (Windows, Linux,
  macOS, Android, iOS, Web) », et `docs/architecture/architecture.md`
  §6 vise le même périmètre — mais `app/` ne contient aujourd'hui que
  les runners `windows/` et `web/`. `linux/`, `macos/`, `android/`,
  `ios/` n'ont jamais été générés. L'intention est documentée depuis
  le début ; l'exécution ne l'a jamais rattrapée.
- **Le mobile n'est pas un nœud, c'est une télécommande.** Décision du
  2026-09-28 (roadmap étape 19 remplacée) : pas de daemon embarqué sur
  Android/iOS — ces plateformes piloteront à distance un
  `onionbit-daemon` desktop via REST/SSE, comme le fait déjà `app/` en
  web. Aucun parcours d'appairage n'a été conçu pour cette réalité ;
  une app mobile qui demande de saisir à la main une clé API et une
  adresse IP serait un premier contact dégradant pour exactement le
  public (mobile) le plus sensible à l'ergonomie.

Rien de tout cela n'est un bug isolé — c'est une interface qui a
correctement livré 15+ fonctionnalités sur un primitif visuel qui n'a
jamais été repensé. ADR-0020 a déjà fait ce travail de mise à plat
pour le *vocabulaire* du projet (« ce n'est plus un client BitTorrent,
c'est un écosystème ») ; l'interface ne le reflète pas encore
visuellement — à l'écran, elle reste une liste de téléchargements
avec des onglets annexes.

## Décision

### 1. Cadrage : ce que « feuille blanche » couvre

« Repartir d'une feuille blanche » porte sur trois couches précises,
pas sur l'application entière :

- **Concerné** : le design system (`core/theme/` → `core/design/`),
  la coquille/navigation (`core/layout/`), les widgets de présentation
  par fonctionnalité (`features/*/presentation/`), l'architecture de
  l'information (IA des réglages, de la navigation), les runners de
  plateforme manquants.
- **Non concerné** : les couches `domain/` et `data/` de chaque
  feature (`Download`, `MessagingConversation`, les repositories
  REST…) sont testées, conformes à l'API et ne portent aucune dette
  visuelle — les réécrire sans raison violerait le principe « une
  fonctionnalité = un propriétaire » (AGENTS.md) et romprait des
  intégrations qui fonctionnent. `core/api/`, `core/config/`,
  `core/platform/` (abstractions desktop/web déjà propres, import
  conditionnel par `dart.library.io`) sont conservés à l'identique.
- **Non concerné** : le choix du framework. Flutter reste la seule
  techno qui couvre les huit cibles demandées (Windows x64/ARM64,
  Linux, macOS, Web, iOS, Android téléphone/tablette) depuis **une
  seule base de code** — voir §4 et Alternatives rejetées.
- **Non concerné** : `onionbit-api`/`onionbit-core`. Zéro endpoint
  n'est modifié par cette décision (la seule proposition d'ajout —
  appairage mobile — est isolée en §8, différée à sa propre
  étape/revue).

« Feuille blanche » signifie donc : **le système visuel et la
coquille applicative sont dessinés sans contrainte d'héritage** (plus
aucune ligne issue de l'app de référence eMule), pas que le dépôt
`app/` est supprimé et reconstruit en bloc.

### 2. Méthode d'exécution : worktree dédié, toujours livrable

Comme ADR-0017 (`adr17`) et ADR-0020 (`adr20`) : l'ampleur du
changement (toute la couche présentation + quatre nouveaux runners)
justifie un **worktree dédié `adr21`**, isolé de `master` pendant
toute la durée des travaux. Deux règles héritées du workflow du
projet (AGENTS.md, « Workflow d'étape ») s'appliquent néanmoins à
l'intérieur du worktree :

- **Stratégie de l'étrangleur (« strangler fig »), pas de big-bang.**
  Le nouveau design system (`core/design/`) est construit en premier,
  aux côtés de l'ancien, puis chaque écran migre feature par feature
  (découpage en étapes §10, roadmap étapes 70–77). L'application
  reste compilable et testable à chaque étape — jamais de période où
  `app/` ne tourne plus, contrairement à un remplacement intégral
  d'un coup.
- **Suppression finale, pas immédiate.** `core/theme/app_theme.dart`
  et les widgets ad hoc ne sont supprimés qu'une fois toutes les
  références migrées (étape 77) — `grep` à zéro occurrence avant
  suppression, même discipline que le sweep des en-têtes GPL
  d'ADR-0020 §7.

### 3. Socle technique : ce qu'on garde, ce qu'on ajoute

**Conservé** (aucun signal de dette à ce niveau — changer coûterait
sans bénéfice) : `flutter_riverpod` (déjà en v3), `go_router`, le
pipeline i18n `gen_l10n` (ADR-0009), le double transport
`http`/`fetch_client` par import conditionnel (ADR-0012),
`file_selector`/`desktop_drop`/`window_manager`/`tray_manager` pour
le desktop.

**Ajouté — en minimisant les nouvelles dépendances** :

- `core/design/` : tokens (couleur, typographie, espacement, rayon,
  élévation, motion) + primitives (boutons, champs, chips, cartes,
  navigation, dialogues), remplaçant `AppTheme`/`AppSpacing`/
  `AppRadii`. Reste dans `app/lib/` (un seul consommateur
  aujourd'hui) — un package Dart séparé (`packages/onionbit_design/`)
  est une **option différée**, à ne lever que si une deuxième app
  apparaît (Alternatives rejetées).
- **Typographie auto-hébergée, zéro requête réseau** : trois polices
  variables OFL embarquées en assets locaux (`assets/fonts/`,
  déclarées dans `pubspec.yaml` → `fonts:`) — **Space Grotesk**
  (titres, identité visuelle), **Inter** (texte d'interface, optimisée
  pour la lisibilité en petite taille), **JetBrains Mono** (hex,
  infohashes, clés, **phrases de seed ADR-0016** — dessinée pour
  distinguer sans ambiguïté `0`/`O` et `1`/`l`, critique quand une
  confusion de caractère sur une seed phrase est irréversible). Le
  package `google_fonts` est délibérément écarté : même avec son mode
  « bundling » documenté, il reste conçu pour aller chercher des
  polices en HTTP par défaut — inutile et contraire à la posture du
  projet (`onionbit-network-policy`) de ne jamais introduire d'appel
  réseau non maîtrisé, fût-il vers un CDN de polices.
- **Pas de nouvelle dépendance pour les icônes/le motion** :
  `flutter_svg` (déjà présent) suffit pour un petit jeu de glyphes de
  marque (indicateur de circuit, badge de confiance) en plus de la
  police d'icônes Material déjà bundlée ; le motion repose sur les
  API d'animation intégrées à Flutter (`AnimatedSwitcher`, `Hero`,
  courbes/durées tokenisées dans `core/design/motion.dart`) — aucune
  librairie d'animation tierce.
- **« Material 3 Expressive » non retenu comme dépendance** : Google
  a présenté cette évolution en 2025, mais l'équipe Flutter a
  explicitement mis en pause son intégration officielle
  (`flutter/flutter#168813` : « we are not actively developing
  Material 3 Expressive right now ») ; le seul portage existant
  (`material_3_expressive` sur pub.dev) impose de remplacer
  `package:flutter/material.dart` par un paquet `material_ui`
  alternatif dans tout le code — changement invasif et non officiel,
  disproportionné pour ~130 fichiers Dart. La décision est
  d'**auteurer notre propre couche expressive au-dessus du Material 3
  stable** (`ColorScheme.fromSeed` + tokens `core/design/` +
  formes/motion maison), sans dépendre d'un fork non soutenu par
  l'amont.
- **Deux nouvelles dépendances, et seulement deux** (appairage
  mobile, §8) : un générateur de QR (rendu pur, pas d'accès caméra)
  côté desktop, un scanner caméra côté mobile — packages Flutter
  établis, utilisés uniquement sur les plateformes concernées
  (gating par plateforme, aucune permission caméra demandée sur
  desktop/web/Linux).
- **Conception « Apple-ready » dès le premier jour, build différé** :
  l'absence d'hôte macOS aujourd'hui ne doit jamais se traduire par
  une présomption implicite « Windows/Android d'abord » dans
  `core/design/`/`core/layout/`. Dès l'étape 70 (§10), le shell
  adaptatif s'appuie sur les hooks d'adaptivité intégrés à Flutter
  (`PageTransitionsTheme` avec `CupertinoPageTransitionsBuilder` sur
  `TargetPlatform.iOS`/`.macOS`, défilement/overscroll adaptatifs via
  `ScrollBehavior`/`ScrollConfiguration`) et suit le patron déjà
  établi par `core/platform/*_native.dart`/`*_stub.dart` pour toute
  future spécificité Apple — de sorte que l'activation réelle de
  macOS/iOS (§4) soit un scaffolding + build de fumée, jamais une
  reprise de conception.

### 4. Matrice de plateformes et état réel aujourd'hui

| Cible | État aujourd'hui | Travail de cet ADR |
| :--- | :--- | :--- |
| Windows x64 | Runner `app/windows/` présent, livré (étape 20) | Migration design system uniquement |
| Windows ARM64 | Pas de build dédié | Détection de l'archi hôte (triple rustc `aarch64-pc-windows-msvc` → `arm64`) dans `build_dist.ps1` — les chemins `build\windows\<arch>\` deviennent paramétrés. `flutter build windows` ne cross-compile pas x64→arm64 : le build ARM64 est natif sur hôte/runner Windows ARM64 (ex. `windows-11-arm`) ; le daemon a déjà `-Target aarch64-pc-windows-msvc` (`build_release.ps1`) |
| Web | Runner `app/web/` présent, livré (Phase 7) | Migration design system + shell adaptatif |
| Linux | Aucun runner | `flutter create --platforms=linux .` + revue de parité des plugins natifs (`window_manager`/`tray_manager`) |
| macOS | Aucun runner | `flutter create --platforms=macos .` — **scaffoldable dès maintenant depuis le worktree Windows** ; build/signature réels différés jusqu'à un hôte macOS (cf. Limites) |
| iOS | Aucun runner | `flutter create --platforms=ios .` — même scaffolding immédiat ; build réel différé (hôte macOS requis) ; modèle « télécommande » (§Contexte), pas de daemon embarqué |
| Android téléphone | Aucun runner | `flutter create --platforms=android .` ; modèle « télécommande » |
| Android tablette | Aucun runner | Même runner Android — layout `expanded`/`medium` du §5 selon la taille réelle, pas un artefact séparé |

Chaque runner généré passe par la revue d'en-têtes GPL (AGENTS.md,
ADR-0020 §7) avant commit — `flutter create` produit des fichiers
natifs (Kotlin/Swift/CMake) qui tombent sous la règle.

**Scaffolding ≠ build.** `flutter create --platforms=...` template
des dossiers ; cette génération est indépendante de l'hôte (confirmé :
Linux/Windows génèrent valablement des dossiers `ios/`/`macos/`).
Seules la compilation et la signature exigent Xcode, donc un Mac.
Conséquence pour le §10 : les quatre runners manquants sont
scaffoldés **ensemble** dès l'étape 72, mais depuis l'hôte Windows
x64 du chantier, **seul Android** obtient un vrai build de fumée
immédiat (`flutter build apk` — SDK présent, toolchain
cross-host). `flutter build linux` exige un hôte Linux (toolchain
ninja/GTK — pas de cross desktop dans l'outil Flutter), le Windows
ARM64 un hôte Windows ARM64 (cf. ligne ci-dessus), macOS/iOS un hôte
macOS (local ou CI, ex. runner `macos-latest`). Ces trois validations
sont reportées à leurs hôtes respectifs sans bloquer la suite du §10.

### 5. Shell adaptatif : des breakpoints déclarés aux layouts canoniques

Les quatre paliers de `breakpoints.dart` sont enfin distingués par un
layout propre — alignés sur les layouts canoniques (« list-detail »,
« supporting pane ») documentés par Material 3 pour ce problème
exact, plutôt que réinventés :

- **`compact`** (téléphone portrait) : `NavigationBar` basse, une
  colonne, détail en bottom sheet — proche du comportement actuel,
  repris visuellement.
- **`medium`** (téléphone paysage, tablette portrait, fenêtre desktop
  étroite) : `NavigationRail` **repliée** (icônes seules) + une
  colonne de contenu ; la navigation vers un détail **pousse** un
  nouvel écran (pas de deuxième panneau — la largeur ne le permet pas
  proprement).
- **`expanded`** (tablette paysage, laptop) : `NavigationRail`
  **étendue** (icônes + libellés) + layout **liste+détail à deux
  panneaux** (ex. téléchargements : liste à gauche, détail à droite ;
  messagerie : conversations à gauche, fil à droite) — le premier
  vrai layout tablette du projet.
- **`large`** (desktop ≥ 1600dp) : sidebar fixe complète (évolution
  de l'actuelle `AppSidebar`) + option d'un troisième panneau
  contextuel (ex. le Privacy HUD, §8).

Chaque page de `features/*/presentation/pages/` se scinde en un
sélecteur de layout (inchangé en logique, un simple `switch` sur
`AppBreakpoint`) + des vues par palier qui partagent les **mêmes
providers Riverpod** — la présentation seule varie, jamais l'état ni
les règles métier (continuité de la clean architecture déjà en
place).

### 6. Entrées : tactile, clavier, souris

- **Cibles tactiles ≥ 48×48dp partout**, y compris desktop
  (l'affordance de survol peut rester visuellement compacte — ex. les
  `ActionChip` denses de `settings_page.dart` — mais la zone de
  hit-test ne descend jamais sous le minimum Material).
- **Clavier desktop/web généralisé.** Les `CallbackShortcuts` déjà
  présents dans `downloads_page.dart` (Ctrl+A, Échap, Espace, Suppr,
  F2) deviennent le patron d'un système transverse
  (`Shortcuts`/`Actions` globaux dans la coquille : Ctrl/Cmd+F
  recherche, Ctrl/Cmd+N ajout, navigation par flèches dans les
  listes) + ordre de focus explicite (`FocusTraversalGroup` par
  panneau) — au lieu d'un câblage qui ne couvre qu'une feature sur
  six.
- **Affordances souris conditionnelles** : survol/curseur/tooltips
  actifs uniquement quand un pointeur fin est détecté (`MouseRegion`,
  `PointerDeviceKind`) — jamais sur tactile pur, pour éviter les
  info-bulles fantômes sur tablette.
- **Palette de commandes (Ctrl/Cmd+K)**, desktop/web uniquement,
  repliée dans l'onglet Recherche sur tactile : recherche globale
  floue sur les téléchargements, contacts/conversations et sections
  de réglages. Aucune nouvelle dépendance — généralisation du
  mécanisme déjà présent dans `settings_page.dart`
  (`_SectionEntry.keywords`) en un registre `CommandCatalog` partagé
  par feature (`core/command/`).
- Le glisser-déposer reste desktop/web (`desktop_drop`, déjà
  conditionnel) — repris visuellement dans le nouveau système de
  motion (overlay de zone de dépôt animé sur toute la coquille, au
  lieu de l'état actuel).

### 7. Direction visuelle « premium 2026 »

- **Sombre par défaut, clair soigné — pas un à-côté.** Le domaine
  (anonymat, confidentialité) justifie un sombre confiant comme thème
  par défaut ; le thème clair est dérivé des **mêmes tokens**, jamais
  retouché séparément (source unique de vérité, contrairement à la
  dérive actuelle).
- **Identité de marque étendue** : les couleurs `#6C2EA6` (seed) et
  `#4FD8E0` (tertiaire) sont conservées — déjà sur la marque — mais
  complétées par une palette sémantique documentée
  (succès/alerte/danger/info, paliers de surface/élévation) au lieu
  de s'en remettre uniquement aux valeurs par défaut de
  `ColorScheme.fromSeed`.
- **Motif visuel « couches d'onion »**, utilisé avec retenue : formes
  concentriques pour le Privacy HUD (§8 — chaque saut de circuit =
  une couche traversée, correspondance directe avec le nom du
  produit) et pour un indicateur de chargement de marque réservé aux
  moments clés (pas chaque petit spinner inline, qui reste Material
  standard) — un langage de formes cohérent plutôt qu'un gadget
  isolé.
- **États vides/erreur comme moments soignés** :
  `core/widgets/empty_state.dart`/`error_state.dart` se limitent
  aujourd'hui à une icône 48px + titre + message — upgrade en
  micro-illustrations de marque via le pipeline SVG existant
  (`assets/branding/`, `flutter_svg`), sans changer leur API
  (`icon`/`title`/`message`/`action`) ni les écrans qui les
  consomment.
- **Un guide de style vivant dans l'app** : route de développement
  `/_style-guide` (masquée en release sauf flag), qui rend chaque
  token/composant — documente le système ET sert de cible aux tests
  golden (§9). C'est la correction structurelle de la cause racine du
  Contexte : sans document vivant, le design dérive ; avec lui, toute
  dérive future casse un test golden avant de casser l'œil.

### 8. Parcours utilisateur premium

- **Réglages par paliers**, pas une liste plate : les 15 sections
  actuelles se regroupent en catégories repliables — « Essentiels »
  (apparence, dossier de téléchargement par défaut, bande passante,
  identité) visible d'emblée, « Réseau & anonymat » /
  « Automatisation » / « Avancé & daemon » repliées par défaut. Rien
  n'est supprimé ni rendu moins accessible (la recherche §6 saute
  directement dans une section repliée) — seule la première
  impression change.
- **Privacy HUD ambiant** : un indicateur persistant (chip dans la
  barre du haut, qui s'étend en panneau) montrant en continu la
  posture d'anonymat — à partir des champs déjà modélisés dans
  `features/diagnostic/domain/diagnostic_models.dart::CircuitInfo`
  (`goalHops`/`actualHops`, `verifiedHops` — le mid hex de chaque
  saut réellement traversé, dans l'ordre —, `exitFlags`, `state`),
  déjà consommés par le panneau Diagnostic mais enterrés dans un
  onglet secondaire. Les rendre ambiants transforme un indicateur de
  debug en moment de confiance signature du produit — dans l'esprit
  de l'icône oignon de Tor Browser, mais informatif plutôt que
  binaire.
- **Appairage mobile par QR code** : puisque Android/iOS pilotent à
  distance un daemon desktop (Contexte) plutôt que d'héberger un
  nœud, le premier lancement mobile n'est pas une création d'identité
  mais un appairage. Réglages → Connexion (desktop) affiche un QR ;
  l'app mobile scanne et se connecte — alternative radicalement plus
  premium que la saisie manuelle d'une adresse IP et d'une clé API
  sur un clavier tactile. Le QR encode un **jeton d'appairage à
  courte durée de vie et usage unique**, jamais la clé API long terme
  brute — ce jeton nécessite un petit ajout d'API, isolé et soumis à
  sa propre revue (§9).
- **Onboarding qui enseigne l'écosystème, pas seulement BitTorrent** :
  les états vides des trois services (partage de fichiers, messagerie,
  identité — vocabulaire ADR-0020) remplacent l'actuel silence/icône
  générique par une invitation à découvrir les deux autres services
  quand on n'utilise que le premier.

### 9. Qualité, accessibilité, non-régression

- **Tests golden** (`flutter test`, `matchesGoldenFile`) par
  composant de `core/design/` et par palier de breakpoint pour les
  écrans à fort trafic (téléchargements, messagerie, réglages) —
  nouvelle porte dans `scripts/verify_all.ps1`, aux côtés de
  `flutter analyze`/`flutter test` existants.
- **Accessibilité comme critère d'acceptation, pas un bonus** : audit
  `Semantics`/tooltip sur chaque composant `core/design/`, navigation
  lecteur d'écran (TalkBack/VoiceOver/NVDA) testée manuellement avant
  qu'une étape du §10 soit cochée (même discipline que « validation
  manuelle » déjà exigée par AGENTS.md).
- **Contraste vérifié, pas évalué à l'œil** : un test dédié
  (`test/design/contrast_test.dart`) calcule les ratios WCAG AA pour
  chaque paire de tokens couleur du thème clair et sombre.
- **i18n inchangé dans son mécanisme** (ADR-0009) : la réorganisation
  de l'IA des réglages (§8) impose une passe de réorganisation des
  clés `app_en.arb`/`app_fr.arb`, toujours gardée par
  `scripts/check_i18n.ps1`.
- **Scripts étendus progressivement**, jamais d'un coup :
  `verify_all.ps1` gagne chaque nouvelle cible de build
  (`flutter build linux`/`macos`/`apk`/`ipa`, `windows
  --target-platform windows-arm64`) au moment où l'étape
  correspondante du §10 livre, pas en anticipation.
- **Seule dépendance croisée identifiée** : le jeton d'appairage
  mobile (§8) nécessite un endpoint minimal côté `onionbit-api`
  (émission + validation d'un jeton court, usage unique). Traité
  comme partie de l'étape 76 (§10), avec sa propre revue de
  sécurité — jamais comme un détail UI.

### 10. Découpage en étapes (Phase 13 de la roadmap, étapes 70–77)

Fixé dans `docs/plans/roadmap.md` à l'acceptation — le détail
d'exécution (bancs de validation, ajustements rencontrés en route)
continue d'y vivre ; cet ADR n'en garde que le squelette :

- **Étape 70.** Fondations `core/design/` (tokens, polices
  auto-hébergées, guide de style `/_style-guide`) + harnais de tests
  golden — aucun écran existant visuellement modifié.
- **Étape 71.** Coquille adaptative v2 (`medium`/`expanded`/`large`
  réellement distincts) + système clavier/focus/raccourcis
  transverse.
- **Étape 72.** Scaffolding des quatre runners manquants (Linux,
  macOS, Android, iOS) + cible Windows ARM64. Build de fumée immédiat
  sur **Android** depuis l'hôte Windows x64 (`flutter build apk` ;
  `kotlin.incremental=false` requis si projet/pub cache sur lecteurs
  distincts). Linux/macOS/iOS/Windows ARM64 restent scaffoldés et
  prêts : chacun exige son hôte natif (Linux pour `flutter build
  linux`, macOS pour Darwin, Windows ARM64 pour l'archi — cf. §4) —
  sans bloquer les étapes suivantes.
- **Étape 73.** Migration Téléchargements + Recherche vers le nouveau
  design system/shell.
- **Étape 74.** Migration Messagerie (+ palette de commandes v1).
- **Étape 75.** Réorganisation par paliers des Réglages + extraction
  du Privacy HUD depuis Diagnostic.
- **Étape 76.** Onboarding identité repensé + appairage mobile par QR
  (avec l'endpoint API dédié du §9).
- **Étape 77.** Portes accessibilité/golden/contraste dans
  `verify_all.ps1` ; suppression d'`AppTheme`/widgets ad hoc une fois
  toute référence éteinte (zéro occurrence `grep`).

## Alternatives rejetées

- **Garder Material 3 par défaut, sans surcouche** : trop générique
  pour « premium » — c'est précisément l'absence de couche d'auteur
  qui a produit le thème « emprunté » actuel.
- **Changer de framework** (natif par plateforme, React Native, .NET
  MAUI, Tauri+web, Compose Multiplatform) : aucun ne couvre les huit
  cibles demandées depuis une seule base aussi bien que Flutter — MAUI
  ne couvre ni Linux ni le Web, React Native n'a pas de support
  desktop mûr, Tauri n'a pas de cible mobile, Compose Multiplatform
  est moins mature que Flutter sur iOS/desktop. Rejeté : le coût de
  huit bases de code (ou de combinaisons partielles) dépasse de loin
  celui d'un design system construit sur l'existant.
- **Adopter `material_3_expressive` (pub.dev)** : impose de remplacer
  `package:flutter/material.dart` par `material_ui` dans tout le
  code, pour une fonctionnalité explicitement mise en pause côté
  Flutter officiel (`flutter/flutter#168813`). Rejeté au profit d'une
  couche expressive maison au-dessus du Material 3 stable.
- **Changer de state management (Bloc/Provider/GetX) ou de routeur** :
  `flutter_riverpod` 3 et `go_router` 18 sont déjà des choix récents
  et idiomatiques — aucun signal de dette à ce niveau ; un changement
  ici casserait des features qui fonctionnent sans bénéfice visuel.
- **Big-bang** (geler `app/`, livrer la refonte complète d'un coup à
  la fusion du worktree) : viole le principe « implémenter → tester →
  cocher » du projet, laisse l'app non livrable pendant toute la
  durée du chantier. Rejeté au profit de la stratégie de l'étrangleur
  du §2.
- **Package de design séparé dès le départ**
  (`packages/onionbit_design/`) : overhead de gestion de version non
  justifié tant qu'une seule app consomme le design system —
  réévaluer si une deuxième surface apparaît.
- **`google_fonts` en mode HTTP (comportement par défaut)** :
  contraire à la posture anti-fuite réseau du projet
  (`onionbit-network-policy`) — écarté au profit de polices
  auto-hébergées en assets (§3).
- **Encoder la clé API brute dans le QR d'appairage** : une clé API
  longue durée visible dans un QR photographiable est un risque
  disproportionné pour un gain d'ergonomie — écarté au profit d'un
  jeton court, usage unique (§8/§9).

## Limites assumées

- **macOS et iOS exigent un hôte macOS pour builder/signer** — cet
  ADR ne lève pas cette contrainte d'outillage ; une CI macOS (ou un
  Mac de build dédié) est un prérequis externe à la **validation**
  de l'étape 72 (et de toute vérification ultérieure spécifique à ces
  cibles), pas une conséquence de cette décision. Ce report ne porte
  que sur le build : la **conception** (§3 « Apple-ready dès le
  premier jour », shell adaptatif §5, scaffolding §4) intègre
  macOS/iOS dès l'étape 70, pour qu'activer ces cibles plus tard soit
  une formalité et jamais une refonte.
- **Le modèle « télécommande » mobile n'est pas remis en cause ici** —
  cet ADR dessine l'UX qui en découle (appairage QR, §8) mais ne
  revisite pas la décision du 2026-09-28 (pas de daemon embarqué sur
  Android/iOS) ; si ce modèle change, l'onboarding mobile devra être
  revu en conséquence.
- **L'audit d'accessibilité reste largement manuel** (lecteurs
  d'écran, contraste automatisé seulement sur les couleurs, pas sur
  les combinaisons réelles de polices/tailles) — pas d'outillage
  d'audit automatisé de bout en bout mis en place par cet ADR.
- **Les 15 sections de réglages ne diminuent pas en nombre** — cet
  ADR réorganise leur présentation (paliers repliables), il ne réduit
  pas la surface fonctionnelle, pilotée par les ADR respectifs
  (0015–0019).
- **La migration feature par feature implique une période de
  cohabitation visuelle** (écrans migrés / non migrés coexistants) —
  assumée et bornée par l'ordre du §10 (fondations puis écrans à fort
  trafic en premier) plutôt qu'éliminée.

## Conséquences

- **Fait à l'acceptation (2026-10-09)** : statut passé à Acceptée ;
  worktree dédié `adr21` créé (branche `adr21`, §2) ; **Phase 13**
  ouverte dans `docs/plans/roadmap.md` (étapes 70–77, détaillées à
  partir du squelette du §10). Aucune ligne de code encore modifiée.
- **Reste à livrer, étape par étape, dans le worktree `adr21`** :
  - `app/pubspec.yaml` : ajout des deux dépendances de pairing
    (génération/scan QR, gating par plateforme) à l'étape 76 ; aucune
    autre dépendance de runtime nouvelle. Polices ajoutées comme
    assets locaux (`assets/fonts/`), pas comme paquet (étape 70).
  - Scaffolding `linux/`, `macos/`, `android/`, `ios/` sous `app/`
    (étape 72) — chaque fichier généré passe la revue d'en-têtes GPL
    (AGENTS.md) avant commit.
  - `scripts/verify_all.ps1`, `scripts/build_dist.ps1`,
    `scripts/build_release.ps1` étendus cible par cible, au rythme
    des étapes livrées (jamais toutes d'un coup).
  - Un endpoint minimal d'appairage côté `onionbit-api` (étape 76) —
    seule évolution backend de cet ADR, avec sa propre revue de
    sécurité avant implémentation.
  - `app/README.md`/`docs/architecture/architecture.md` §6 : mise à
    jour une fois les runners effectivement scaffoldés (étape 72) —
    ces documents décrivent déjà l'intention cible, pas encore l'état
    réel.
- **Pendant l'implémentation** : `app/` reste livrable et testable
  (`flutter analyze`/`flutter test`/`flutter build web`/`flutter
  build windows`) à chaque étape cochée — aucune régression
  fonctionnelle tolérée entre deux étapes.
- **Risque assumé** : effort étalé sur les huit étapes 70–77 (toute
  la couche présentation + quatre runners neufs) — mitigé par le
  séquencement (fondations d'abord, écrans à fort trafic ensuite,
  plateformes neuves dès que le design system est stable).
- **Une fois les étapes 70–77 livrées** : suppression de
  `core/theme/app_theme.dart` et des widgets ad hoc remplacés,
  `app/README.md`/`docs/architecture/architecture.md` reflètent les
  huit cibles réellement livrées (et non plus seulement visées), et
  la mention « implémentée » s'ajoute au statut de cet ADR (même
  convention qu'ADR-0012/ADR-0017).
