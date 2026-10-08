# ADR-0020 — Définition du logiciel : écosystème P2P anonyme et anti-censure

Statut : **Proposée** (2026-10-08, deux tours de revue externe
intégrés : §7 en-têtes GPL réécriture complète + oracle ancré
`/*` compris, « portable » remplace « souveraine », propagation
complétée — README refondu, SVG, i18n, Cargo.toml) — en attente
d'approbation **et de la fin d'implantation d'ADR-0019** (§7).
Document publié comme les autres ADR — il est la référence de
vocabulaire que les fichiers publics citeront (§Conséquences) :
le garder local créerait des liens morts sur GitHub.
ADR documentaire : aucun code impacté avant acceptation. La
propagation du vocabulaire au README, `AGENTS.md`,
`docs/architecture/architecture.md`, `docs/THREAT-MODEL.md`,
`app/pubspec.yaml`, `app/lib/l10n/*.arb`, en-têtes GPL et
descriptions `Cargo.toml` intervient **à l'acceptation**
(§Conséquences).

## Contexte

OnionBit est né comme « portage Rust natif du daemon Tribler »
(`AGENTS.md`) avec pour promesse publique « Anonymous BitTorrent
client » (README). Ce cadre initial a été progressivement dépassé
par les capacités livrées (ou, pour ADR-0019, en cours de livraison
— Phase 12, étapes 64–67 livrées à la date de rédaction) :

| ADR | Capacité ajoutée | Pourquoi elle déborde « client BitTorrent » |
| :--- | :--- | :--- |
| ADR-0011 | Messagerie anonyme e2e sur circuits | un utilisateur peut ne jamais télécharger de torrent |
| ADR-0015 | Couche d'extension OnionBit-only (hello signé, attestations de curateurs, ledger bilatéral, enveloppes OBF) | un second réseau superposé au legacy, sans équivalent Tribler |
| ADR-0016 | Identité portable (graine racine, phrase 24 mots, invité, scellé au repos, `OBID`) | le produit n'est plus lié à un usage mais à une identité transportable |
| ADR-0017 | Transport furtif anti-censure (bridges, Elligator2, silence sous probing) | la menace visée dépasse « cacher un téléchargement » : rester joignable sous censure |
| ADR-0018 | Zones de stockage portables public/privé (`OBD`/`OBM`) | un espace utilisateur chiffré, pas une fonction torrent |
| ADR-0019 *(en cours)* | Groupes et pièces jointes | messagerie complète, concurrente des messageries dédiées |

Trois glissements structurels en résultent :

1. **BitTorrent n'est plus le centre** — c'est un service parmi
   d'autres sur la toile onion (messagerie, confiance, identité).
   Définir le projet par lui induit en erreur l'utilisateur qui
   vient pour la messagerie ou l'anti-censure.
2. **« Portage de Tribler » est réducteur** — Tribler reste la
   référence protocolaire et le réseau legacy compatible, mais les
   extensions OnionBit-only (messagerie, trust, identité, stealth)
   forment un ensemble autonome que Tribler ne voit même pas.
3. **Le périmètre de menace a changé** — de « anonymiser un
   téléchargement » à « communiquer et échanger de façon furtive
   sous observation et censure ».

Sans définition officielle, le vocabulaire diverge déjà : le README
dit « port », les docs disent « extension », l'UI parle de
« confidentialité ». Cet ADR fige la définition canonique.

## Décision

### 1. Définition canonique

**FR (texte long, référence des docs) :**

> OnionBit est un écosystème pair-à-pair anonyme et résistant à la
> censure — une toile onion sans serveur qui porte ses propres
> services : partage de fichiers, messagerie et identité portable.

**EN (tagline README/publique) :**

> OnionBit is an anonymous, censorship-resistant peer-to-peer
> ecosystem — a serverless onion fabric carrying its own services:
> file sharing, messaging and portable identity.

**Version courte (badge, une ligne) :** « écosystème P2P anonyme et
résistant à la censure » / "anonymous, censorship-resistant P2P
ecosystem".

**Formule filiation :** « né comme portage Rust de Tribler, devenu
écosystème autonome — compatible avec Tribler, défini par ses
extensions. BitTorrent est le premier service, pas le produit. »

### 2. Modèle en couches du vocabulaire

Aucun terme unique ne couvre les quatre facettes ; la définition
repose sur un **modèle en couches** — chaque terme a une portée
exacte et n'en déborde pas :

| Terme | Désigne | Portée d'usage |
| :--- | :--- | :--- |
| « **OnionBit** », « l'écosystème » | le tout : daemon + UI + CLI + réseau + services | pitch, README, présentation — jamais pour un composant seul |
| « la **toile onion** » / "onion fabric" | l'infrastructure partagée : overlay IPv8, circuits multi-sauts, hidden services, transport stealth | docs techniques — ce sur quoi les services roulent |
| « le **réseau OnionBit** » | la population de nœuds OnionBit-only (ext, stealth, messagerie) | par opposition au « réseau legacy » = Tribler 8.x |
| « les **services** » | la suite applicative : BitTorrent anonyme, messagerie, identité, confiance | roadmap, périmètre fonctionnel |
| « la **posture anti-censure** » | le mode `stealth` + kill switch + silence sous probing | la face « plateforme anti-censure » — toujours qualifiée, jamais garantie absolue |
| « mode **legacy** » | la compatibilité filaire Tribler 8.x | config, docs interop |

Note de vocabulaire : « toile onion » / "onion fabric" est le terme
de marque retenu — il distingue OnionBit de « the Tor network ».
« **Overlay** » reste admis comme synonyme technique en interne
(c'est le terme de la littérature P2P) ; "fabric" évoque Hyperledger
chez certains lecteurs, compromis assumé au profit de la marque.

### 3. Ce que OnionBit n'est pas (bornes)

- **Pas un client BitTorrent** — BitTorrent est un service de
  l'écosystème ; un utilisateur messagerie peut ne jamais ouvrir
  un torrent.
- **Pas un portage** — la fidélité protocolaire Tribler est une
  propriété vérifiée du mode legacy, pas l'identité du projet.
- **Pas un fork** — aucune ligne de code commune avec Tribler ;
  la parenté est protocolaire, pas logicielle.
- **Pas un VPN ni un client Tor** — pas de transport TCP générique,
  pas de sortie arbitraire vers Internet (exit policy stricte,
  `onionbit-network-policy`).
- **Pas « anonyme » au sens absolu** — les propriétés sont mesurées
  par des bancs (`docs/security/threat_model.md` — FR, bancs de
  validation, distinct du `docs/THREAT-MODEL.md` public EN qui
  décrit les propriétés d'anonymat ; `fingerprinting.md`), jamais
  décrétées ; la corrélation de trafic reste hors périmètre assumé.
- **Pas incensurable** — le mode furtif supprime les marqueurs
  statiques, pas le volume ni le timing ; le bootstrap par liens
  de pont reste un problème social (ADR-0017 §Limites).

### 4. Positionnement parmi les proches

| Projet | En une ligne | Rapport à OnionBit |
| :--- | :--- | :--- |
| Tor | anonymise un transport TCP générique | complémentaire, pas concurrent — OnionBit anonymise des services précis sur sa propre toile |
| I2P | réseau overlay hébergeant des services | le plus proche en structure ; OnionBit ajoute le BitTorrent natif interopérable et l'identité portable |
| Briar / Session | messageries sans serveur | OnionBit inclut la messagerie dans un ensemble plus large (fichiers, identité, confiance) |
| Freenet / GNUnet | plateformes anti-censure de publication | OnionBit est centré échange/communication live, pas publication statique |
| Tribler | ancêtre protocolaire, réseau compatible | référence de fidélité + population legacy ; OnionBit ajoute messagerie, trust, identité, stealth |

### 5. Règles de vocabulaire (docs, UI, code commenté)

- **« anonyme »** : toujours qualifié (« anonymat multi-sauts »,
  « circuits onion ») — jamais « anonymat total » ou « garanti ».
- **« anti-censure »** : « résistant à la censure » en texte long ;
  jamais « incensurable » ni « indétectable ».
- **« furtif »** : « sans marqueur statique, mesuré par banc » ;
  jamais « invisible ».
- **« écosystème »** : réservé au tout — un crate ou un service seul
  n'est pas un écosystème.
- **« sans serveur »** : signifie « aucun serveur applicatif requis »
  (DHT, pairs, circuits) — pas « sans infrastructure ».
- **« portable »** : réservé à l'identité/stockage transportable
  entre devices (`OBID`, `OBV1`, zones `@…`) — pas à « léger ».
  C'est le terme canonique d'ADR-0016 ; « **souverain** » /
  "sovereign" est proscrit des textes publics — renvoie au
  concept SSI/DID (W3C) qu'OnionBit n'implémente pas.
- **« overlay »** : synonyme technique interne de « toile onion »
  admis — terme de la littérature P2P (voir note du §2).

### 6. Règle de périmètre pour les futures fonctionnalités

Une fonctionnalité entre dans le périmètre du projet si elle :

- **(a)** sert un service porté par la toile onion (partage,
  messagerie, découverte, confiance, identité) ; ou
- **(b)** renforce l'anonymat, la résistance à la censure ou
  l'autonomie de l'identité.

Est hors périmètre par défaut : tout ce qui exige un serveur
central, un compte, un numéro de téléphone ; tout ce qui casse la
compatibilité legacy sans mode dédié (précédent ADR-0017 : le
stealth est disjoint, jamais hybride) ; toute promesse de sécurité
non mesurable par un banc.

### 7. En-têtes GPL — réécriture complète, formulation figée

~400 fichiers sources portent la notice « This file is part of
OnionBit - a Rust port of the Tribler daemon. » (`crates/` ≈ 221,
`app/lib/`, `scripts/`, `fuzz/`). Le descripteur « a Rust port »
contredit frontalement le §3 (« pas un portage ») dans l'artefact
le plus lu du projet — le code lui-même. La divergence ne peut pas
être laissée en place : **tous les en-têtes sont réécrits**.

Nouvelle formulation — la notice se résume au nom, aucun
descripteur (elle ne devra jamais suivre la définition) :

```text
This file is part of OnionBit.
Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
SPDX-License-Identifier: GPL-3.0-or-later
```

Exécution :

- **Un seul diff mécanique dédié**, via un script Python
  (UTF-8 natif — jamais PowerShell : la corruption d'encodage des
  accents est documentée dans `AGENTS.md`). Le script remplace
  uniquement la ligne de descripteur, dans les variantes de
  commentaires existantes (`//` Rust/Dart, `#` PS1/Py/CMake/sh,
  `<!-- -->` Markdown, `/* * */` C — `fuzz/sancov_shim.c` :
  remplacer la seule ligne de descripteur par `/* This file is part
  of OnionBit.` laisse le bloc valide). Attention aux `.sh` et aux
  `.py` (`fuzz_asan.sh`, `package_posix.sh`, `scripts/interop/`) :
  la notice est en **ligne 2, après le shebang** — le script doit
  la repérer par contenu, pas par position.
- **Périmètre** : **balayage de tout le dépôt** — le sweep a
  révélé des porteurs hors des dossiers évidents (`.gitattributes`,
  `.github/workflows/ci.yml`, `app/l10n.yaml`,
  `app/windows/**/CMakeLists.txt`, `docs/plans/README.md`). Le
  script traite donc tout fichier portant la notice, en excluant
  seulement `vendor/` (Apache-2.0 librqbit, inchangé par règle),
  `target/`, `app/build/`, `app/.dart_tool/`, `.git/`.
- **Vérification de non-régression** : le commit s'accompagne d'un
  grep oracle **ancré sur la forme d'en-tête** (pas sur la phrase
  nue — sinon l'oracle échouerait sur cet ADR même, qui la cite en
  prose, et sur tout futur document qui en parlerait) :

  ```text
  (//|#|<!--|/\*)\s*This file is part of OnionBit\s*-\s*a Rust port
  ```

  (le `/*` couvre le bloc C de `sancov_shim.c` — sans lui le gate
  laisserait passer le seul fichier C du dépôt).

  → **0 occurrence** hors `vendor/` ; plus un second balayage
  `part of OnionBit` confirmant que toutes les notices restantes
  sont de la nouvelle forme. Ajouté en gate dans
  `scripts/verify_all.ps1` pour verrouiller durablement le §5.
- **Timing** : la réécriture atterrit **après l'implantation
  complète d'ADR-0019** — ainsi tout fichier créé pendant la
  Phase 12 naît avec la nouvelle notice et le sweep + gate
  vérifient aussi ces documents récents.
- **Worktree dédié** : l'exécution se fait sur un worktree
  (convention `adr17` d'ADR-0017 — p. ex. `adr20`) : la
  quasi-totalité des fichiers du dépôt étant touchée, le diff
  mécanique est isolé du travail en cours et les conflits évités.
- **Fichiers nouveaux** : même formulation dès l'acceptation.
- `AGENTS.md` (règle « En-tête GPL ») est amendé à l'acceptation
  pour refléter la nouvelle formulation.
- La filiation Tribler n'est pas effacée : elle reste affirmée au
  README (section License, « derivative work »), dans
  `docs/reference_tribler/` et dans cette décision même — la
  notice légale n'a plus à la porter seule.

## Alternatives rejetées

- **Garder « client BitTorrent anonyme »** : réducteur et trompeur
  — ne couvre ni la messagerie, ni l'identité, ni le stealth.
- **Garder « port de Tribler »** : la filiation est un fait
  historique et une compatibilité filaire, pas une définition ;
  « port » suggère une dépendance que les extensions n'ont pas.
- **« Fork de Tribler »** : factuellement faux — aucun code partagé.
- **Un seul des quatre termes** : chacun capte une facette
  (produit / infrastructure / menace / applications) — le modèle
  en couches du §2 est la synthèse retenue.
- **« Plateforme » seul** : évoque un serveur central — contraire
  au « sans serveur » constitutif.
- **Néologisme de catégorie** (type « onion-suite ») : du jargon
  marketing, contraire à l'objectif « simple et clair ».
- **Renommer le projet** : la marque OnionBit porte déjà l'oignon —
  le problème est la définition, pas le nom.
- **Garder les en-têtes existants inchangés** (notices figées,
  seuls les nouveaux fichiers corrigés) : laisse ~400 occurrences
  de « a Rust port of the Tribler daemon » dans le code —
  l'artefact le plus lu du projet — en contradiction frontale
  avec le §3. Rejetée : le diff est mécanique et scriptable en
  toute sécurité (Python UTF-8, §7).
- **« Overlay » comme terme de marque** : générique dans la
  littérature P2P ; conservé comme synonyme technique interne
  uniquement (§2).
- **« Identité souveraine » dans la tagline** : colliding avec le
  concept SSI/DID (W3C) qu'OnionBit n'implémente pas ; le terme
  canonique du projet est « identité portable » (ADR-0016).

## Conséquences

- **À l'acceptation**, propagation de la définition et du
  vocabulaire du §2/§5/§7 :
  - `README.md` : **refonte complète** — le document est aujourd'hui
    structuré autour du cadran « client BitTorrent » (hero «
    Anonymous BitTorrent client », « A full Rust port of Tribler »,
    « What is OnionBit? » centré moteur). Réagencement plutôt que
    réécriture : la prose des sections services (messagerie,
    extension layer, identité, stealth) est déjà conforme au §5 et
    conservée ; c'est l'ordre et le cadran qui changent — le modèle
    en couches du §2 (définition → usages → mécanisme → garanties →
    filiation) :
      1. **Hero** : badge du §1 « Anonymous, censorship-resistant
         peer-to-peer ecosystem » + traduction grand public «
         One network, no servers: share files, message privately
         and own a portable identity — all over multi-hop onion
         circuits. » — « BitTorrent » sort du hero, il appartient
         aux services ;
      2. **What is OnionBit?** : 3–4 lignes d'écosystème + table
         « You want to… → OnionBit gives you » — chaque ligne dit
         le bénéfice utilisateur d'abord, le mécanisme ensuite ;
      3. **The fabric** : la toile onion en 30 secondes
         (`you → relay → relay → exit → swarm`, « each hop only
         knows its neighbors ») — les bullets mécanisme actuels
         y migrent ;
      4. **Services** : une section par service — File sharing
         (BitTorrent anonyme), Messaging (+ groupes et pièces
         jointes), Identity (ADR-0016), Trust & ext layer
         (ADR-0015), Stealth (ADR-0017) ;
      5. **Honest limits** : le blockquote « Status: beta » promu
         en section — les bornes affichées sont un argument de
         confiance, pas une note de bas de page ;
      6. **Interop** : « Proven interoperability » + filiation —
         la formule §1 version EN : « Born as a Rust port of
         Tribler, OnionBit grew into its own ecosystem — still
         wire-compatible with Tribler 8.x. » ; badge optionnel
         `Tribler-compatible` (porte l'interop sans l'inscrire
         dans la définition) ;
      7. Screenshots / Architecture / Getting started / Roadmap /
         Contributing / License inchangés ; **screenshot
         messagerie** ajouté à la grille quand l'étape 68 livre
         l'UI — la grille actuelle ne montre que le côté torrent,
         ce qui contredirait le nouveau cadran.
    Squelette de travail rédigé : `docs/plans/
    readme_draft_adr0020.md` (local — `docs/plans/` non tracké) ;
    la propagation est la substitution du squelette, pas une
    réécriture à froid ;
    Tagline du repo GitHub (champ « About ») **et topics** alignés
    manuellement (`p2p`, `anonymity`, `censorship-resistant`…) —
    réglages GitHub, pas des fichiers ;
  - `assets/github-social.svg` (+ `social-preview-1280x640.png`
    régénéré) — la bannière porte l'ancien cadran (« Anonymous
    BitTorrent client, native in Rust », « Tribler port · … »).
    Retexte arrêté : ligne tagline = version badge du §1 «
    Anonymous, censorship-resistant P2P ecosystem » ; ligne
    descripteurs = les services + licence « File sharing ·
    Messaging · Portable identity · GPL-3.0 ». « Tribler-compatible »
    est volontairement absent de la bannière — la filiation informe
    (README) mais ne définit plus ;
  - `assets/logo-horizontal.svg` (+ `logo-horizontal.png` régénéré) :
    la ligne uppercase « ANONYMOUS BITTORRENT IN RUST » → «
    ANONYMOUS P2P ECOSYSTEM » (23 car., rentre sans retoucher la
    taille) ; `icon.svg` ne porte qu'un commentaire interne
    factuel (« cœur BitTorrent ») — inchangé ;
  - `AGENTS.md` : première phrase (« Portage Rust natif du daemon
    Tribler » → écosystème + filiation) **et** règle « En-tête GPL »
    (nouvelle formulation du §7) ;
  - `docs/architecture/architecture.md` : préambule court renvoyant
    à cet ADR ;
  - `docs/THREAT-MODEL.md` : « this port » → reformulation
    « OnionBit's circuits » ;
  - `app/pubspec.yaml` : `description` actuelle « A new Flutter
    project. » → « OnionBit UI — client for the OnionBit
    ecosystem. » ;
  - `app/lib/l10n/app_en.arb` / `app_fr.arb` : audit des chaînes
    visibles parlant de « torrent client »/« port » — déclenche
    `scripts/check_i18n.ps1` ;
  - `crates/*/Cargo.toml` : `onionbit-api` (« daemon Tribler Rust »
    → « daemon OnionBit ») ; `onionbit-format` garde « Formats de
    fichiers Tribler » — fait protocolaire exact, pas une
    identité ;
  - **en-têtes GPL : réécriture complète** des ~400 notices selon
    le §7 — un seul commit dédié via script Python UTF-8,
    `vendor/` exclu.
- Cet ADR devient la **référence de vocabulaire** : tout nouveau
  document, ADR ou commentaire de code se conforme au §5.
- **Aucun changement de comportement ni de filaire** : la
  réécriture des en-têtes ne touche que des commentaires ; ni
  code, ni protocole, ni config ne bougent.
