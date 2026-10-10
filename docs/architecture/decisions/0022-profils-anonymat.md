# ADR-0022 — Profils d'anonymat prédéfinis : « Legacy », « Full anonyme », « Personnalisé »

Statut : **Implémentée** (2026-10-09 — worktree `adr22` basé sur
`adr21`, fusion `master` incluse ; étapes 78–81 livrées :
`PrivacyProfile` + presets dans `onionbit-core`, `GET`/`PUT
/api/privacy/profile` + gardes settings/invité dans `onionbit-api`,
sélecteur `PrivacyProfileSwitch` sidebar/rail/feuille compacte dans
`app/`, banc `bench_profiles.ps1` 13/13 oracles dont PCAP « 0
datagramme legacy » en `full`). Acceptée le même jour après une
revue externe — corrections intégrées : `obf_enabled` reclassé clé
froide, gardes hybride/invité documentées comme *ajouts* et non
réutilisations, validateur de combinaison porté au niveau
`DaemonConfig`, cycle de redémarrage vérifié sans endpoint nouveau.
Dépendance d'interface résolue : la coquille v2 d'ADR-0021
(`AppSidebar`, Privacy HUD) a été fusionnée avant l'étape 80.

## Contexte

La posture d'anonymat d'OnionBit n'est pas un réglage : c'est une
**combinaison** d'une vingtaine de clés éparpillées dans
`configuration.json`, ajoutées au fil des ADR :

| Clé | Section | Défaut actuel | Introduite par |
| :--- | :--- | :--- | :--- |
| `anonymity_enabled` / `number_hops` / `safeseeding_enabled` | `download_defaults` | `true` / `1` / `true` | parité Tribler (Phase 3) |
| `guards_enabled` | `tunnel_community` | `true` | ADR-0010 |
| `messaging_enabled` / `messaging_hops` / `messaging_groups_enabled` | `tunnel_community` | `true` / `1` / `true` | ADR-0011/0019 |
| `ledger_enabled` / `ledger_enforce` / `messaging_consent_ledger` | `tunnel_community` | `true` / `false` / `false` | ADR-0015 |
| `enabled` / `obf_enabled` / `ledger_enabled` / `hello_jitter_pct` | `ext` | `true` / `false` / `true` / `25` | ADR-0015 |
| `enabled` / `role` / `bridges` / `cover_traffic` | `stealth` | `false` / `client` / `[]` / `false` | ADR-0017 |
| `default_area` | `storage` | `public` | ADR-0018 |
| `at_rest` | `identity` | `false` | ADR-0016 |

Trois problèmes concrets en découlent :

- **Aucune posture n'est exprimable.** L'utilisateur voit des
  interrupteurs isolés dans Réglages (15 sections, ADR-0021 §Contexte)
  mais aucune réponse à « suis-je anonyme, et à quel niveau ? ».
  Le Privacy HUD (ADR-0021 §8) rend la posture *visible* — rien ne la
  rend *actionnable*.
- **Les combinaisons utiles ne sont pas évidentes.** Activer
  `ext.obf_enabled` sans `stealth` masque les trames ext mais laisse
  la couche legacy identifiable (ADR-0017 §Contexte) ; activer
  `stealth` sans `stealth.bridges` coupe tout ; passer
  `stealth.enabled=true` sans `ipv8.enabled=false` est un refus ferme
  au démarrage (`session.rs` — combinaison hybride rejetée). Un
  utilisateur qui compose à la main peut produire un daemon qui ne
  démarre plus ou une protection illusoire.
- **Deux régimes filaires disjoints coexistent désormais** : le mesh
  legacy Tribler-compatible (IPv8 clair + tunnels) et le mode furtif
  OnionBit↔OnionBit (ADR-0017 : « impossible d'être à la fois non
  classifiable et interopérable legacy sur le même nœud »). Ce choix
  structurel est aujourd'hui enfoui dans une section « stealth » de
  Réglages, sans avertissement de ses conséquences (interop Tribler
  sacrifiée, redémarrage, prérequis de ponts).

La demande est un **sélecteur de profil à trois positions dans la
barre latérale** — posture globale, pas réglage par téléchargement
(le `anon_hops` par download reste orthogonale) :

- **« Legacy »** : les défauts actuels, compatibles Tribler — ce que
  le logiciel fait au premier démarrage.
- **« Full anonyme »** : toutes les options d'anonymat et
  d'obfuscation activées, trafic **exclusivement OnionBit↔OnionBit**
  (transport furtif ADR-0017, interop legacy sacrifiée).
- **« Personnalisé »** : combinaison libre choisie dans Réglages.

## Décision

### 1. Un profil est un *preset matérialisé*, pas une couche d'override

Le profil sélectionné est persisté dans une nouvelle section racine
de `configuration.json` :

```jsonc
{
  "privacy": {
    "profile": "legacy"   // "legacy" | "full" | "custom"
  }
}
```

`privacy.profile` est une **intention**. Appliquer un profil
matérialise son preset : le daemon réécrit l'ensemble des clés
couvertes (table §3) via le **même chemin atomique** que
`POST /api/settings` — merge sur une copie, validation de la
combinaison *résultante*, persistance, notification SSE
`settings_changed`. Une application est tout-ou-rien : un prérequis
manquant refuse la bascule sans laisser la config à moitié mutée
(précédent `identity.at_rest`, `settings.rs`).

Conséquences directes :

- Pas d'état fantôme : `GET /api/settings` reflète toujours la
  réalité effective ; le profil est une vue *dérivée* des clés, pas
  une couche qui les masquerait.
- `custom` ne matérialise rien — le sélectionner conserve les clés
  telles quelles (le preset « personnalisé » est la configuration
  courante).
- **Divergence détectée** : le profil *effectif* exposé par l'API est
  le profil stocké si toutes les clés couvertes égalent son preset ;
  sinon `custom`. Modifier une clé couverte dans Réglages bascule
  donc automatiquement l'affichage sur « Personnalisé » — sans état
  supplémentaire ni synchronisation à maintenir.

### 2. Les trois profils

**`legacy` — le défaut, compatible Tribler.** Exactement les défauts
actuels : mesh IPv8 clair, téléchargements anonymes 1 saut,
messagerie e2e active, communauté ext présente (les nœuds OnionBit se
reconnaissent) mais sans obfuscation négociée ni gate d'admission.
C'est le seul profil qui interopère avec des clients Tribler 8.x.

**`full` — OnionBit-only, posture maximale.** Toutes les mesures
d'anonymat/obfuscation : transport furtif ADR-0017 (le seul régime
réellement non classifiable — `OBF` seul ne masque pas l'existence du
protocole), 3 sauts sur tous les services, zone privée chiffrée par
défaut, ledger enforced. **Prérequis dur : au moins un lien de pont
(`stealth.bridges` non vide)** — un client furtif sans pont est une
enclave vide (ADR-0017 §3) ; la bascule refuse `409
missing_prerequisites` plutôt que de produire un daemon isolé ou un
hybride refusé au démarrage. L'UI intercepte ce refus et propose la
saisie d'un lien `onionbit-bridge://` inline
(`POST /api/stealth/bridges`) puis réessaie.

**`custom` — combinaison libre.** Aucun preset appliqué ; les clés
couvertes restent éditables individuellement dans Réglages. C'est
aussi l'échappatoire pour « posture renforcée sans stealth » (hops 3
+ OBF + ledger_enforce sur le mesh legacy) — volontairement non
offerte comme quatrième preset (Alternatives rejetées).

### 3. Table de correspondance (les seules clés couvertes)

| Clé | `legacy` | `full` |
| :--- | :--- | :--- |
| `ipv8.enabled` | `true` | **`false`** |
| `stealth.enabled` | `false` | `true` |
| `stealth.role` | `client` | `client` |
| `stealth.cover_traffic` | `false` | `true` |
| `download_defaults.anonymity_enabled` | `true` | `true` |
| `download_defaults.number_hops` | `1` | `3` |
| `download_defaults.safeseeding_enabled` | `true` | `true` |
| `tunnel_community.enabled` | `true` | `true` |
| `tunnel_community.exitnode_enabled` | `false` | `false` |
| `tunnel_community.guards_enabled` | `true` | `true` |
| `tunnel_community.messaging_enabled` | `true` | `true` |
| `tunnel_community.messaging_hops` | `1` | `3` |
| `tunnel_community.messaging_groups_enabled` | `true` | `true` |
| `tunnel_community.ledger_enabled` | `true` | `true` |
| `tunnel_community.ledger_enforce` | `false` | `true` |
| `tunnel_community.messaging_consent_ledger` | `false` | `true` |
| `ext.enabled` | `true` | `true` |
| `ext.ledger_enabled` | `true` | `true` |
| `ext.obf_enabled` | `false` | `true` |
| `storage.default_area` | `public` | `private` |

La table vit **une seule fois**, dans `onionbit-core`
(`PrivacyProfile::preset()` — règle « aucune valeur en dur » : les
presets sont des structs de config nommés, pas des littéraux
dispersés) ; ni l'API ni l'UI ne dupliquent la correspondance.

**Jamais touchées par un preset** (données utilisateur ou à flux
dédié) : `stealth.bridges` (prérequis lu, jamais récrit),
`stealth.tuning.*`, `ext.curators`, `identity.*`
(`at_rest` exige son propre flux à mot de passe — en `full`, l'UI le
*recommande* via un indice, sans jamais le basculer), `api.*`,
interfaces/ports, `storage.private_enabled` (la zone privée reste
disponible en `legacy` ; seul le défaut d'ajout change),
`libtorrent.*`, et le `anon_hops` des téléchargements existants
(`number_hops` ne gouverne que les nouveaux ajouts).

Notes de la table :

- `ipv8.enabled=false` × `stealth.enabled=true` sont toujours
  écrites **ensemble** par le preset `full` — l'atomicité du merge
  garantit qu'on ne peut pas persister l'hybride refusé au démarrage.
  Le retour `full → legacy` fait l'inverse dans le même patch.
- `ext.obf_enabled=true` est conservé dans `full` bien qu'ADR-0017
  rende OBF redondant sous stealth : si l'utilisateur revient ensuite
  vers `custom` et désactive stealth, l'obfuscation ext reste en
  place plutôt que de retomber à découvert.
- `ledger_enforce` + `messaging_consent_ledger` en `full` : un nœud
  « anonyme total » qui sert quand même le mesh (relays, sorties
  tunnel dans les circuits) applique l'économie de contribution
  ADR-0015 plutôt que de servir indéfiniment — et le consentement
  messagerie hérite de la gate dette.
- En stealth `client`/`bridge`, RSS, torrent-checker et la session
  BitTorrent directe sont déjà neutralisés par
  `stealth_blocks_direct` (session.rs) — aucune clé
  supplémentaire à couvrir pour ces services.

### 4. API

Deux endpoints dédiés derrière `api_key_auth` (mutation de
configuration sensible — même discipline que `/api/settings`) :

- `GET /api/privacy/profile` →
  `{stored, effective, diverged_keys: [...], restart_pending,
   stealth: {bridges_configured: n}}` — `effective` est la dérivation
  du §1 (`custom` dès qu'une clé couverte diverge du preset stocké) ;
  `diverged_keys` liste les clés qui diffèrent, pour l'affichage «
  personnalisé : hops modifié ».
- `PUT /api/privacy/profile` `{profile}` → merge atomique du preset →
  `{modified, profile, effective, restart_required, applied_keys}` ;
  `409 missing_prerequisites {missing: ["stealth.bridges"]}` si
  `full` sans pont ; `409` si la combinaison résultante viole une
  garde (`at_rest × role` — existante dans `settings.rs` ;
  `stealth × ipv8` — **ajoutée par cette phase** : le refus hybride
  n'existe aujourd'hui qu'au démarrage (`session.rs`), et
  `POST /api/settings` accepte encore de persister une config qui
  refuserait de démarrer — trou pré-existant fermé ici, dans
  `update_settings` comme dans le chemin profil).
- `POST /api/settings` **refuse** désormais l'écriture directe de
  `privacy.profile` — même précédent que `identity.at_rest` : un
  champ-intention qui ne matérialise pas son preset produirait un
  état incohérent (stocké `full`, clés `legacy`).
- En **session invitée**, `PUT /api/privacy/profile` refuse `409
  guest_session` : persister la posture dans le `configuration.json`
  réel contredirait « zéro artefact » d'ADR-0016 — la même garde est
  ajoutée à `POST /api/settings`, où le trou est pré-existant (un
  invité y persiste aujourd'hui dans la config de l'utilisateur).
  Le `GET` reste libre, et le gate identitaire (`pending`/`locked`)
  couvre déjà ces endpoints automatiquement via la whitelist
  existante.

`restart_required` est calculé en diffant les clés *à redémarrage*
modifiées (`ipv8.enabled`, `stealth.*`, `ext.enabled`,
`ext.obf_enabled` — `ExtSettings` est un champ immuable de
l'overlay (`ext.rs`), aucun basculement à chaud n'existe —,
`tunnel_community.enabled`, `messaging_hops`…) — la quasi-totalité
d'une bascule `legacy ↔ full` l'exige ; l'UI l'affiche via le
bandeau de redémarrage existant. Les clés à application à chaud
(`guards_enabled`, `ledger_*`, `number_hops`, `default_area`)
prennent effet immédiatement.

**Cycle de redémarrage** — aucun endpoint nouveau n'est requis, la
mécanique existe déjà :

- la bascule écrit la config **d'abord** (le `PUT` retourne
  `restart_required: true` — la posture cible est persistée, elle
  prend effet au prochain boot) ;
- côté UI, le dialogue propose « Redémarrer maintenant » : `PUT
  /api/shutdown` (parité Tribler) → le SSE tombe → la reconnexion
  appelle `ensureDaemonRunning()`, qui respawn le daemon local
  détaché (chemin déjà emprunté par la bannière « daemon
  injoignable ») → l'app se reconnecte sur le nouveau régime ;
- si l'utilisateur refuse/remet le redémarrage : `GET` expose
  `restart_pending: true` (config persistée ≠ config effective en
  cours — même diff que `apply_runtime_view`) et le badge du
  sélecteur affiche « en attente de redémarrage » — la posture
  *réelle* reste lisible dans le Privacy HUD (circuits réels) ;
- **daemon distant ou headless** (web, mobile pilotant un desktop) :
  l'UI ne peut pas respawn un processus qui n'est pas local — le
  dialogue l'indique (« redémarrez le daemon sur sa machine ») et
  le bouton « Redémarrer maintenant » n'est offert que quand le
  daemon est local (`daemon_launcher` sait le déterminer).

### 5. Interface — sélecteur dans la barre latérale

- `PrivacyProfileSwitch` dans `AppSidebar` (coquille ADR-0021), en
  tête de la colonne de navigation sous le bloc vitesses — la posture
  est un contrôle primaire, pas enfoui dans Réglages : contrôle
  segmenté à 3 positions (icône + libellé :
  `Compatible`/`Full anonyme`/`Personnalisé`), réduit à une icône
  cyclable quand la rail est repliée.
- Palier `compact` (sans sidebar) : le sélecteur vit dans la feuille
  du Privacy HUD (même provider Riverpod — la présentation change,
  jamais la source de vérité, §5 ADR-0021).
- Sélection de `full` : dialogue de **conséquences** avant envoi —
  « interop Tribler sacrifiée, seuls les nœuds OnionBit furtifs sont
  joignables, redémarrage requis » — + saisie inline d'un lien de
  pont si `stealth.bridges` est vide (le `409` déclenche ce même
  dialogue en garde-fou). Après un `PUT` réussi, le même dialogue
  propose « Redémarrer maintenant » quand le daemon est local (flux
  §4 : `shutdown` → respawn par `ensureDaemonRunning`) ou indique
  la marche manuelle pour un daemon distant.
- Le badge du sélecteur reflète `effective`, pas `stored` : une clé
  couverte modifiée à la main → « Personnalisé » s'affiche, et le
  détail (`diverged_keys`) est accessible en tooltip/panneau.
- i18n FR/EN par `gen_l10n` (ADR-0009) ; aucun état de profil
  persisté côté client — le daemon est la seule source de vérité
  (`ui_prefs` ne stocke que l'expansion/collapse existante).

**Textes explicatifs (copy deck, clés `l10n` `privacyProfile*`)** —
la bascule de posture est un choix de sécurité : chaque mode porte
une description courte sous son label, et `full` un avertissement
complet dans son dialogue. Ton visé : factuel sur les compromis,
jamais de promesse d'anonymat absolu (cohérent avec les « Limites »
d'ADR-0017).

| Profil | Label | Description courte (sous-label / tooltip) |
| :--- | :--- | :--- |
| `legacy` | « Compatible » | « Réseau ouvert — compatible Tribler. Circuits anonymes (1 saut), guard nodes. Votre trafic reste identifiable comme du trafic OnionBit/Tribler. » |
| `full` | « Full anonyme » | « OnionBit↔OnionBit uniquement — transport furtif non classifiable, 3 sauts, zone privée chiffrée. Invisible pour le réseau Tribler. Nécessite un pont. » |
| `custom` | « Personnalisé » | « Combinaison libre — réglée dans Réglages → Réseau & anonymat. S'affiche aussi quand une option couverte est modifiée à la main. » |

Dialogue de conséquences à la sélection de `full`
(`privacyProfileFullTitle` / `…Body` / `…BridgeHint`) :

> **Passer en mode Full anonyme ?**
>
> Ce mode coupe toute interopérabilité avec le réseau Tribler
> classique — seuls les nœuds OnionBit en transport furtif resteront
> joignables. Toutes les protections sont activées : transport
> morphé, 3 sauts sur téléchargements et messagerie, zone privée
> chiffrée par défaut, contribution mesurée par ledger.
>
> Un redémarrage du daemon est nécessaire. L'anonymat n'est jamais
> absolu : le volume et le timing de votre trafic restent
> observables.
>
> *(si aucun pont configuré)* Ce mode exige au moins un pont
> OnionBit — collez un lien d'invitation `onionbit-bridge://` :
> `[champ]` — sinon la bascule sera refusée.
>
> `[Annuler]`  `[Ajouter le pont et activer]` / `[Activer]`

Retour `full → legacy` : dialogue allégé (« reconnexion au mesh
Tribler, redémarrage requis ») — pas de champ pont, aucune donnée
(`stealth.bridges`) n'est effacée. Bascule `custom` : pas de
dialogue (aucune clé n'est modifiée).

### 6. Portée explicite

- **Global, jamais par téléchargement** : `anon_hops` par download
  et le choix de zone à l'ajout restent des surcharges orthogonales —
  le profil règle les *défauts* et le *régime filaire*, pas chaque
  torrent. Des profils par download (hors sujet) multiplieraient la
  matrice sans répondre au besoin « posture du nœud ».
- **`network-policy` inchangé** : le kill switch stealth d'ADR-0017
  reste la garantie ultime ; le profil n'ajoute ni ne retire aucun
  chemin réseau, il compose des clés existantes.
- **Rôles stealth `bridge`/`gateway` hors sélecteur** : servir le
  réseau furtif est un choix d'exploitation, pas une posture
  utilisateur — ils restent dans Réglages (et restent incompatibles
  avec `identity.at_rest`, validation existante conservée).

## Alternatives rejetées

- **Couche d'override runtime** (le profil masque les clés sans les
  récrire) : deux sources de vérité divergentes (`GET /api/settings`
  mentirait sur l'état réel) ; la matérialisation par preset garde
  un seul état persisté, lisible et dérivable — rejetée.
- **Un quatrième preset « renforcé sans stealth »** (hops 3 + OBF +
  ledger_enforce sur le mesh legacy) : posture intermédiaire réelle
  mais non demandée ; `custom` la couvre exactement, et multiplier
  les presets fige une ligne arbitraire entre « compatible » et
  « furtif » que l'utilisateur peut déjà composer — rejetée, mais la
  table du §3 rend l'ajout futur trivial.
- **`full` dégradé silencieusement** (appliquer le preset sans
  stealth quand `bridges` est vide) : afficherait « Full anonyme »
  pour un nœud encore classifiable en clair — fausse assurance,
  contraire à l'honnêteté de posture du projet (ADR-0017
  « Limites »). Refus ferme + dialogue de pont, pas de demi-mode
  silencieux.
- **Stealth optionnel dans `full`** (checkbox « transport furtif »
  dans le dialogue) : réintroduit le demi-mode ci-dessus sous un
  nom unique — les deux états auraient le même label pour des
  régimes filaires opposés ; rejetée (celui qui veut « tout sauf
  stealth » a `custom`).
- **Presets définis dans l'UI** (mapping Dart qui écrit via
  `/api/settings`) : dupliquerait la table dans chaque client (app,
  web, futur CLI) et laisserait diverger les postures selon le
  client utilisé ; un seul propriétaire — `onionbit-core` — comme
  pour toute règle métier (AGENTS.md).
- **Dérivation symétrique de l'état effectif** (`clés == preset →
  ce profil`, indépendamment de `stored`) : rendrait `stored` quasi
  redondant, et afficherait « Full anonyme » à un utilisateur qui a
  recréé le preset à la main sans jamais l'avoir demandé — soulevé
  en revue externe, conservé tel quel : l'affichage reflète
  l'**intention déclarée**, pas la coïncidence de valeurs (cohérent
  avec §1 « le profil est une intention »).
- **Profil par téléchargement** : explosion de la matrice (profil ×
  hops × zone × état du circuit actuel) sans cas d'usage « posture »
  ; `anon_hops` par download existe déjà pour le grain fin.
- **Inclure `identity.at_rest` dans le preset `full`** : impossible —
  le scellement `OBSK` exige un mot de passe et son endpoint dédié
  (ADR-0016) ; un preset qui l'écrirait dans l'arbre contournerait
  le refus ferme de `settings.rs`. Recommandé en UI, jamais forcé.

### 7. Variante serveur : `full` + rôle `bridge`/`gateway`

Addendum (2026-10-10, suite au déploiement ADR-0024 §8) — le
preset `full` du §3 est une posture **cliente**. Un nœud d'exploitation
qui *sert* le réseau furtif (bootnode, pont d'infrastructure) applique
la **même table §3 à une exception près** :

| Clé | Preset `full` client | Variante serveur |
| :--- | :--- | :--- |
| `stealth.role` | `client` | `bridge` (ou `gateway`) |
| `stealth.bridges` | requis (prérequis `409`) | non requis — le nœud *est* le pont ; les liens d'autres ponts y sont facultatifs (mesh entre ponts) |
| `stealth.client_allowlist` | — (hors preset) | optionnel : restreint les `client_id` admis |
| `identity.at_rest` | recommandé | **interdit** — un pont doit redémarrer sans surveillance (garde `at_rest × role` existante) |

Conséquences :

- La bascule se fait **manuellement** dans `configuration.json`
  (merge du patch §3 + `role`), jamais via `PUT
  /api/privacy/profile` : le prérequis `stealth.bridges` bloquerait
  la bascule et le preset récrirait `role = "client"`. Les deux
  comportements sont *corrects pour un client* — la variante
  serveur n'a pas vocation à passer par le sélecteur.
- `GET /api/privacy/profile` dérive `effective = "custom"` sur ces
  nœuds (`stealth.role` est une clé couverte divergeant du preset
  stocké) — **attendu et sans correction prévue** : le label
  « personnalisé » reflète fidèlement une posture hors sélecteur.
  Si l'affichage devait un jour distinguer « full serveur » de
  « custom utilisateur », ce serait un quatrième profil dédié, pas
  un réglage de la dérivation.
- `ipv8.enabled=false` reste inconditionnel : un pont émet
  **uniquement** du trafic morphé — y compris vers les clients qui
  le contactent (sinon il déclassifierait le régime de tout le
  monde, ADR-0017 §Limites).
- Les ports UDP stealth et BitTorrent doivent être **figés en
  config** (la sonde `port..=port+10` rend le port imprévisible au
  restart) et publiés au pare-feu — voir ADR-0024 §8 pour le
  déploiement de référence.

## Limites assumées

- **La bascule `full` exige un pont que le logiciel ne peut pas
  fournir** — le bootstrap social des liens d'invitation est la
  limite connue d'ADR-0017 ; le sélecteur ne la supprime pas, il la
  rend explicite (dialogue) au lieu de la cacher dans Réglages.
- **Redémarrage quasi systématique** d'une bascule `legacy ↔ full`
  (`ipv8.enabled`, `stealth.*`) : signalé par `restart_required` et
  le bandeau existant ; pas de bascule à chaud du régime filaire —
  reconstruire la stack sous le pied d'un mesh vivant est hors
  périmètre.
- **Les téléchargements existants gardent leurs sauts** : repasser
  en `full` ne re-saute pas un téléchargement déjà à 1 saut — le
  changement de `number_hops` vaut pour les ajouts suivants
  (cohérent avec la sémantique `download_defaults` partout).
- **`custom` est une dérivation, pas une combinaison nommée** : deux
  configs « personnalisées » très différentes portent le même label
  — compensé par `diverged_keys` exposé à l'API/UI, pas par des
  sous-profils.
- **La garde invité sur `POST /api/settings` peut sur-verrouiller** :
  un invité réglant RSS/watch_folder en mémoire verrait un `409`
  sec — si l'implémentation trouve un flux légitime cassé, le repli
  est « applique en mémoire, ne persiste pas » plutôt que refus
  (check explicité à l'étape 79, soulevé en revue externe).
- **La posture affichée n'est pas une garantie** : « Full anonyme »
  reste borné par les limites ADR-0017 (volume observable,
  corrélation globale hors périmètre, étranglement UDP aveugle) —
  le wording UI évite « anonymat total garanti » au profit de la
  description du régime (« OnionBit-only, transport furtif »).

## Conséquences

- **Fait à l'acceptation (2026-10-09)** : statut passé à Acceptée
  après revue externe ; **Phase 14** ouverte dans
  `docs/plans/roadmap.md` (étapes 78–81) ; plan détaillé
  `docs/plans/roadmap_adr0022.md` ; copy deck figé en §5. Cible
  `master` après fusion d'ADR-0021 (le sélecteur vit dans la
  coquille v2) ; si le besoin presse, worktree `adr22` basé sur
  `adr21` (convention « Worktree dédié » ADR-0020 §7). Aucune ligne
  de code encore modifiée.
- **Étape 78.** `onionbit-core` : `PrivacyProfile` (énum +
  `preset()` table §3 + `effective(cfg)` dérivation divergente),
  section `privacy` de `DaemonConfig`, validateur de combinaison
  porté au niveau `DaemonConfig` (règles aujourd'hui dispersées :
  `stealth × ipv8` et `at_rest × role` ne vivent qu'en
  `session.rs`/`settings.rs`) + prérequis `stealth.bridges` pour
  `full` ; tests unitaires mapping/divergence/refus.
- **Étape 79.** `onionbit-api` : `GET`/`PUT /api/privacy/profile`,
  refus de `privacy.profile` via `POST /api/settings`, gardes
  **ajoutées** `stealth × ipv8` (trou pré-existant : persistable
  aujourd'hui) et `guest` sur les deux chemins (settings + profil),
  `restart_required`, erreurs typées `409`, SSE `settings_changed` ;
  tests hostiles (bascule sans pont, hybride forcé via l'arbre
  générique, session invitée, profil inconnu).
- **Étape 80.** `app/` : `PrivacyProfileSwitch` sidebar + variante
  Privacy HUD compacte, dialogue conséquences/pont
  (`POST /api/stealth/bridges` inline puis retry), badge effectif +
  `diverged_keys`, chip redémarrage ; i18n FR/EN, tests widgets +
  golden (étape 77).
- **Étape 81.** Validation : banc inter-démons de bascule
  (`legacy→full→legacy`, isolement OnionBit-only vérifié — un pair
  Tribler/pyipv8 ne voit plus le nœud en `full`), divergence →
  `custom` en live, entrée cataloguée `bancs_tests.md`,
  `docs/security/fingerprinting.md` mis à jour (la posture est
  désormais un état visible), CHANGELOG, statut → implémentée.
