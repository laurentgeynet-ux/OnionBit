# Procédure de release signée `OBUP` (ADR-0032)

Les releases sont des **artefacts importés** — jamais téléchargées par le
daemon (la sonde `versions/check` n'existe pas : décision vie privée).
La signature Ed25519 transporte la confiance ; le canal (site officiel,
`.onion` d'un pair, clé USB) est indifférent.

## 1. La clef de release — une fois

Sur une machine **dédiée** (idéalement hors-ligne — la clef privée n'a
rien à faire sur la machine de build ni dans le repo) :

```powershell
onionbit-cli gen-release-key --out release-key.bin
```

Affiche :

```
clef privee  : release-key.bin
clef publique: <64 hex>
key_id       : 0x....
```

- `release-key.bin` : 32 octets, droits 0600 sous Unix — **jamais
  commité, jamais dans les logs, jamais en argv hex** (l'historique
  shell le retiendrait). `sign-release --key -` accepte stdin.
- La **clef publique** remplace le placeholder dans
  `crates/onionbit-crypto/src/release.rs` :

```rust
pub const RELEASE_PUBKEY: [u8; 32] = [0xAB, 0xCD, /* ...32 octets... */];
```

**Garde automatisée** : `build_dist.ps1 -ZipRelease` **refuse** de
produire un zip si `RELEASE_PUBKEY == [0;32]` (un tel bundle serait
muet pour les updates — `verify_strict` rejette tout, sans alerte).
`-AllowPlaceholderReleaseKey` existe pour les tests de la chaîne,
jamais pour une release publiée.

## 2. Construire et signer

```powershell
# 1) Bundle (machine de build — ne signe PAS, decision deliberee)
pwsh -File scripts/build_dist.ps1 -ZipRelease

# 2) Signature (machine de release — la clef y vit seule)
onionbit-cli sign-release --dir dist\OnionBit --key release-key.bin `
    --version 1.3.0 --target windows-x64
# → dist\OnionBit\update.obup ; affiche clef publique + key_id :
#   comparer visuellement a RELEASE_PUBKEY avant publication.
```

`sign-release` énumère le payload de façon déterministe (séparateurs
`/`, tri canonique — même bundle → même manifest quelle que soit la
machine), exclut `state/`, `windows.old/`, `*.obup`, et écrit
`update.obup` à la racine du bundle (`--out` pour changer).

Options : `--min-supported 1.2.0` (instances plus vieilles refusées),
`--rollback` (autorise un downgrade — signé, à utiliser avec
parcimonie), `--next-key <fichier-pub>` (rotation, §4).

Distribuer le **dossier complet** (ou un zip le contenant) :
`update.obup` + les fichiers du payload forment l'artefact.

## 3. Installation côté utilisateur

1. Copier le contenu de la release dans `state/update/incoming/`
   (fichiers + `update.obup`) — ou équivalent UI à venir ;
2. `POST /api/versioning/import` → vérification signature + `key_id`
   + anti-downgrade + `sha256` de chaque fichier → staging
   `state/update/payload/` + `staged.obup` (`GET /api/versioning/update`
   pour le statut) ;
3. Au redémarrage, le launcher **re-vérifie** signature + hashes des
   fichiers stagés, puis swappe `windows/` → `windows.old/` et
   `payload/` → `windows/` avant de lancer le nouveau.

## 4. Rotation de clef (`next_key`)

Une release signée par la clef courante peut déclarer sa successeure :

```powershell
onionbit-cli gen-release-key --out release-key-2.bin
# → noter la « clef publique » hex affichee, la sauver dans
#   release-key-2.pub (un fichier texte hex, ou 32 octets bruts)
onionbit-cli sign-release ... --next-key release-key-2.pub
```

L'import persiste la successeure dans `state/release_key.next` —
la release suivante, signée par la nouvelle clef, est acceptée sans
rebuild.

## 5. Compromission de la clef privée

`next_key` ne protège **qu'avant** compromission : une clef volée
signe n'importe quoi, *y compris sa propre successeure* — la rotation
signée est morte à partir de ce point. Procédure honnête :

1. Générer une nouvelle clef hors-ligne ;
2. Embarquer la nouvelle `RELEASE_PUBKEY` dans les sources, rebuild ;
3. Distribuer le nouveau bundle **hors-bande** (canal authentifié
   externe — site, empreinte publiée sous clé de secours) ;
4. Supprimer `state/release_key.next` côté installations si la
   successeure compromettante a pu être importée ;
5. Communiquer : les utilisateurs ne doivent plus faire confiance aux
   blobs signés par l'ancienne clef.

## 6. Rollback manuel (utilisateur)

Une release signée mais cassée (crash au démarrage) se revient à la
main — `windows.old/` est conservé un premier run pour ça :

1. Fermer OnionBit complètement (icône systray → Quitter) ;
2. Dans le dossier d'installation : supprimer le dossier `windows` ;
3. Renommer `windows.old` en `windows` ;
4. Relancer OnionBit.

Pas de rollback automatique : un binaire qui démarre et plante en
boucle ne doit pas osciller entre versions.
