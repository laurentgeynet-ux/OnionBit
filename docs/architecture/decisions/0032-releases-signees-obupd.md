# ADR-0032 — Releases signées `OBUPD` (import manuel, pas de sonde)

- Statut : **Proposée** (2026-10-18) — jalon de décision : étape 141
  (spike format + signature + vérification).
- Contexte : `backlog_priorites.md` « Releases signées +
  reproductibles + canal d'update » — **seul vecteur de
  compromission systémique** : une mise à jour compromise compromet
  toutes les installations d'un coup, clefs chiffrées comprises
  (ADR-0031 protège au repos, pas contre un binaire patché).

## Contexte

OnionBit est distribué en bundle portable (`build_dist.ps1` →
`OnionBit/` + `windows/` + `state/` + lanceurs). Aujourd'hui, une
mise à jour = remplacer le dossier `windows/` à la main — **sans
aucune vérification**. Quiconque distribue un bundle modifié (forum,
miroir, clé USB, faux site) installe un backdoor : la base est
chiffrée mais c'est le binaire qui détient la clef.

Contrainte fondatrice : **`versions/check` a été retiré par
décision vie privée** (`daemon_config.rs` §versioning — interroger
un serveur de releases = télémétrie « une instance OnionBit tourne
à cette IP »). Le canal d'update ne peut donc **pas** être un
fetch automatique : c'est un artefact **importé** par l'utilisateur
(site officiel, miroir, sneakernet, ou `.onion` d'un pair — le
transport est indifférent, seule la signature compte).

## Décision

### 1. Format `OBUP` (convention blobs portables)

```
"OBUP" (4o magic) | version u8 (1) | key_id u64 LE (8o)
| sig ed25519 (64o) | manifest bencode
```

Le manifest est un dict bencode canonique :

- `v` : version de la release (semver string, ex. `"1.2.0"`) ;
- `target` : cible (`"windows-x64"`, extensible) ;
- `ts` : timestamp de création (unix) ;
- `files` : liste ordonnée `[{p: "windows/OnionBit.exe",
  sha256: <32o>, size: u64}, ...]` — chemin **relatif à la racine
  du bundle**, normalisé `/`, interdit `..`/absolu ;
- `min_supported` (option) : version semver minimale exigée —
  refus d'installer si l'instance est plus ancienne (fenêtre de
  migration DB/format, évite les sauts impossibles) ;
- `rollback` (option, défaut `false`) : autorise le downgrade —
  doit être signé explicitement ;
- `notes` (option) : chaîne libre affichée avant confirmation.

Signature : `ed25519` sur `manifest` brut (les octets bencode,
domaine préfixé `onionbit/update-manifest/v1` concaténé avant
signature pour éviter tout réemploi cross-protocol — même
convention que `onionbit/onion-bind/v1`).

### 2. Racine de confiance

Clef Ed25519 de release dédiée — **offline** (machine de build,
jamais dans le repo). La clef publique est embarquée en `const`
dans `onionbit-launcher` et `onionbit-daemon` (même octets, source
unique `RELEASE_PUBKEY` dans `onionbit-crypto`). `key_id` = les 8
premiers octets de `SHA-256(pubkey)` : permet la rotation sans
ambiguïté.

**Rotation** : le manifest peut porter `next_key` (32o) — une
release signée par la clef courante peut **déclarer sa
successeure** ; l'updater stocke la nouvelle clef acceptée dans
`state/release_key.next` au premier démarrage post-swap. Compromis
documenté : chaîne de clefs comme `onion-bind`, mais la clef de
secours reste le re-téléchargement manuel.

### 3. Point d'insertion : deux vérifications, pas une

Le daemon vérifie à l'import **et** le launcher re-vérifie au
swap — la seconde vérification couvre le cas « state dir tripatouillé
entre import et redémarrage » (le launcher est la dernière porte
avant de remplacer `windows/`).

**Import (daemon, `POST /api/versioning/import`)** :

1. lit le blob `OBUP` (upload ou chemin local) ;
2. vérifie `key_id` ∈ clefs connues (embarquée ∪ `release_key.next`),
   signature Ed25519 sur le manifest ;
3. parse manifest → `v` semver **strictement >** version courante
   (sauf `rollback=true` signé + confirmation UI explicite) ;
4. vérifie `sha256` + `size` de **chaque** fichier du payload (ZIP
   ou arborescence adjacente au blob) ;
5. écrit le payload vérifié dans `state/update/` + le blob `OBUP`
   original comme `state/update/staged.obup` (preuve de provenance
   re-vérifiée au swap) ;
6. répond `{staged: true, version}` — l'UI propose « appliquer au
   redémarrage ».

**Swap (launcher, au démarrage, avant de lancer le payload)** :

1. `state/update/staged.obup` présent → re-parse + re-vérifie la
   signature (même clef embarquée) et les `sha256` des fichiers
   stagés ;
2. tout vérifié → swap atomique : `windows/` → `windows.old/`,
   `state/update/payload/` → `windows/` ;
3. lancement du nouveau payload ; au prochain démarrage sans
   staging, `windows.old/` est effacé (best-effort, puis rollback
   manuel documenté : renommer `windows.old` si le nouveau build
   plante — la fenêtre est le premier run).

Échec à toute étape → rien n'est touché, erreur remontée
(`staged.obup` conservé pour diagnostic, jamais appliqué partiellement).

### 4. Ce que OBUPD n'est PAS

- **Pas de fetch réseau** — la décision vie privée tient :
  `versions/check` reste absent, aucune sonde, aucun callback.
  Le canal de *distribution* (site, `.onion`, USB) est hors scope :
  la signature transporte la confiance, pas le canal.
- **Pas d'auto-install** — l'import est un geste utilisateur ;
  le swap attend le prochain lancement.
- **Pas Authenticode** — signature Windows éditeur orthogonale
  (certificat payant, centralisé) ; `OBUP` couvre le contenu, pas
  le shim OS.
- **Pas de reproductibilité** dans ce jalon — le bit-for-bit
  reproductible est un chantier séparé (build hermétique) ; `files`
  sha256 fournit déjà la base d'attestation.

### 5. Menaces couvertes / non couvertes

| Couvert | Non couvert |
|---|---|
| Bundle modifié en distribution (miroir, USB, MITM) | Clef de release compromise (mitigation : rotation `next_key`, hors-ligne) |
| Downgrade vers version vulnérable | Déni de service « refus de mettre à jour » (l'ancien binaire marche) |
| Payload incomplet/tronqué | Compromission de la machine de build (reproductibilité = chantier séparé) |
| `state/` tripatouillé après import | Premier install — la confiance initiale vient du canal, non signable |

## Plan

Voir `docs/plans/roadmap_adr0032.md` : **141** = spike format OBUP
+ signature/vérification Ed25519 + clef embarquée + blob rond-trip
(GO/NO-GO) ; **142** = création côté release (`cli sign-release`) ;
**143** = import daemon (`/api/versioning/import`, staging) ;
**144** = swap launcher (re-vérification, rename atomique, purge
`windows.old`) ; **145** = durcissement (anti-rollback,
`min_supported`, rejeu, erreurs typées) ; **146** = docs + procédure
release + clôture.

## Verdict du spike (étape 141, 2026-10-18) — GO

- `onionbit-crypto::release` : `RELEASE_PUBKEY` (placeholder
  `[0;32]`, fail-closed), `release_key_id` (`SHA-256(pk)[..8]`),
  `sign/verify_update_manifest` domainés, `accepted_keys` =
  embarquée + `state/release_key.next` ;
- `onionbit-format::obupd` : blob `OBUP` (magic+version+key_id+sig+
  manifest bencode canonique `BTreeMap`), `parse`/`build`/`verify`,
  `ObupdError` typé (`Malformed`/`UnknownKeyId`/`SigInvalid`/
  `BadPath`), bornes `MAX_OBUP_SIZE`/`MAX_OBUP_FILES`/
  `MAX_OBUP_PATH` avant tout parse ;
- **Trouvaille du spike** : `VerifyingKey::verify` accepte les
  clefs faibles — `[0;32]` (point identité) validait la signature
  `(0,0)` : le placeholder aurait été *fail-open*. Correction :
  `verify_strict` obligatoire (rejette clefs non-canoniques et
  points faibles) — le test `clef_placeholder_ne_verifie_rien` le
  vérifie directement ;
- **Launcher** : `onionbit-format`+`onionbit-crypto` liées,
  `check_staged_update` câblé — **285 Ko → 422 Ko** (+147 Ko pour
  Ed25519+bencode embarqués, acceptable pour la seconde porte) ;
- 11 tests verts : traversal Windows complet (`..\`, `C:\`,
  `\\?\`, `a//b`, `a/./b`, `/abs`, `../`), doublon de chemin,
  `UnknownKeyId` vs `SigInvalid` distingués, borne taille avant
  parse + troncation, chaîne `next_key` (A→k1 declare k2 acceptée,
  C→k3 non chaînée refusée), cross-domaine rejeté, manifest altéré
  post-signature rejeté.
