# ADR-0016 — Identité portable : export/import chiffré (solution intérimaire)

Statut : **partiellement implémentée** (2026-10-06 ; architecture
cible décidée 2026-10-08) — l'export/import `OBID` et le durcissement
du fichier local sont livrés ; la graine BIP39 + chiffrement « at
rest » opt-in sont décidés, en attente d'implantation.

## Contexte

L'identité OnionBit est la clé secrète IPv8 (`LibNaCLSK:`) stockée en
clair dans `state_dir/ipv8_keypair.bin`, générée au premier lancement.
Constats :

- **Non nomade** : impossible de retrouver son identité (contacts,
  confiance ADR-0015, coffre `OBV1`) sur un autre appareil sans copier
  le fichier brut à la main.
- **Peu protégée** : le fichier héritait des permissions par défaut du
  répertoire, écrit non atomiquement.
- **Contrainte daemon** : le service doit démarrer sans interaction —
  un chiffrement « at rest » exigeant un mot de passe au boot bloquerait
  le démarrage headless.

Références des systèmes sans compte ni serveur : phrase BIP39
(wallets), recovery words (Session), fichier `secret` copié à la main
(Secure Scuttlebutt), identité non exportable (Briar — choix maximal
incompatible avec notre coffre de contacts), clé fichier (Tor hidden
services).

## Décision (intérimaire)

**Un daemon = un `state_dir` = une identité.** Pas de profils multiples
pour l'instant.

1. **Fichier local durci, non chiffré.** `ipv8_keypair.bin` reste en
   clair (démarrage sans mot de passe) mais écrit **atomiquement**
   (tmp + rename — un crash ne laisse jamais de fichier tronqué) avec
   permissions `0600` sous Unix ; sous Windows l'ACL du profil
   utilisateur protège déjà `state_dir`.
2. **Export protégé par mot de passe.** C'est le blob qui voyage (USB,
   mail, presse-papiers) qui mérite la protection :
   `OBID‖v‖sel‖nonce‖ciphertext‖tag` — argon2id RFC 9106 (m = 19 Mio,
   t = 2, p = 1) → ChaCha20-Poly1305. Sel et nonce aléatoires par
   export. Sans mot de passe, export hex brut assumé.
3. **API** (sous `api_key_auth`) :
   - `GET /api/identity` → `{public_key}` — jamais de matériel privé ;
   - `POST /api/identity/export` `{password?}` → `{key, encrypted}` ;
   - `POST /api/identity/restore` `{key, password?}` → valide le
     `LibNaCLSK:`, remplace le fichier atomiquement, répond
     `{restart_required: true}`.
4. **Restart obligatoire.** La clé est liée aux communautés en cours —
   on ne mute pas l'identité à chaud (sinon l'API annoncerait une clé
   alors que les paquets restent signés par l'ancienne).
5. **Chaîne de migration** : export `OBID` (device A) → restore
   (device B) → redémarrage → import du coffre `OBV1` → contacts +
   alias restaurés sous la même identité.

## Décision cible (2026-10-08)

**Graine racine HKDF + phrase BIP39 24 mots + chiffrement « at rest »
opt-in réservé au rôle client.** Multi-profils : toujours différé.

### Modèle de clés

- `identity_seed.bin` : 32 octets aléatoires, racine unique de
  l'identité ; écriture atomique tmp+rename, `0600` (même discipline
  que `stealth_bridge.key`).
- Dérivation HKDF-SHA256 (`hkdf` déjà en dep) à domaines séparés :
  - `onionbit/identity/ipv8-crypt/v1` → `crypt_sk` X25519 ;
  - `onionbit/identity/ipv8-sign/v1` → seed ed25519 ;
  - `onionbit/identity/bridge/v1` → `stealth_bridge.key` — la phrase
    couvre aussi la clé de pont ADR-0017, jusqu'ici non exportable.
- `ipv8_keypair.bin` / `stealth_bridge.key` deviennent des **caches
  dérivés** : régénérés au boot depuis la graine, plus source de
  vérité (compat : en présence des deux, la graine dérivée gagne).

### Phrase de récupération BIP39

- 32 o d'entropie → 264 bits → 24 mots. Wordlist anglaise officielle
  BIP39 vendored (2048 mots, domaine public) — encode/decode maison
  ~100 lignes, pas de crate externe.
- `GET /api/identity/recovery_phrase` derrière `api_key_auth` (le
  détenteur de `api.key` est déjà racine de confiance loopback) ;
  `POST /api/identity/restore` accepte la phrase en alternative au
  blob `OBID`.
- Pas de passphrase « 25e mot » en v1 (complexité UX pour un gain
  redondant avec le chiffrement at-rest).

### Migration

- Install existante sans graine = identité **legacy** : tout continue
  de fonctionner, `GET /api/identity` ajoute `seeded: false`.
- Pas de conversion sans re-key — changer de clé = nouvelle identité
  (perte de la confiance ADR-0015). Documenté, jamais forcé.
- Nouvelles installs : graine générée au premier démarrage.

### Premier boot : gate « identité » (aucune clé jetable sur le fil)

- Premier démarrage **spawné par l'UI** (`--first-run-gate` passé par
  `daemon_launcher`) : le daemon monte l'API puis s'arrête en état
  `identity_pending` — aucune session, aucune signature. L'UI impose
  le choix « nouvelle identité » / « restaurer » (phrase BIP39 ou
  `OBID`) ; la session ne démarre qu'ensuite.
- **Daemon headless** (pont stealth, service) : sans le flag, la
  graine est générée silencieusement au premier boot comme
  aujourd'hui — jamais de blocage sans surveillance.
- Justification anonymat : une clé jetable signée pendant quelques
  secondes laisserait un churn de pubkey observable (et un
  `client_id` transitoire vers les ponts stealth) ; le seul matériel
  cryptographique visible doit être l'identité définitive.
- Même mécanisme que le mode locked ci-dessous : « API up,
  `Session::start` différé » — un seul gate, deux déclencheurs.

### Mode invité (session éphémère)

Troisième résolution du gate `identity_pending` : **« Session
invitée »** — graine générée **en mémoire uniquement**, aucun fichier
identité écrit ; l'identité cesse d'exister à la fermeture du daemon.

- `GET /api/identity` → `mode: "guest"`, `persistent: false` ;
  l'UI affiche un bandeau permanent « rien n'est conservé ».
- Cohérence de persistance : invité force `database.enabled=false`
  (état `:memory:` existant) — sinon l'historique téléchargements
  contredirait la promesse. Coffre `OBV1`, ledger et contacts en
  mémoire seulement.
- Réseau : l'invité est un nouveau `pk` à chaque session — toujours au
  budget `intro_seed`, jamais de réputation ADR-0015. Un pont stealth
  avec `client_allowlist` le rejettera (attendu, documenté).
- Pas de surface d'abus nouvelle : un invité n'a pas plus de
  capacité qu'une réinstallation (pk frais = inconnu, pas de
  confiance acquise).
- Headless : sans effet sans UI ; flag `--guest` possible plus tard —
  v1 : résolution du gate UI uniquement.
- Distinction clé : l'identité éphémère **choisie** (invité) n'a rien
  à voir avec la clé jetable **involontaire** que le gate élimine —
  la première est une promesse utilisateur, la seconde était une
  fuite de churn.

### Chiffrement « at rest » (opt-in, client uniquement)

- `identity.at_rest = true` → graine stockée chiffrée (`OBSK` :
  magic + argon2id RFC 9106 m=19 Mio/t=2/p=1 → ChaCha20-Poly1305,
  réutilise `keyblob.rs`).
- **Boot locked** : le daemon monte l'API (`api.key` reste en clair —
  sinon deadlock de déverrouillage), diffère `Session::start`, répond
  `409 identity_locked` sur les endpoints dépendants de l'identité ;
  `POST /api/identity/unlock {password}` (rate-limité) démarre la
  session à retardement. Le statut locked reste derrière `api_key_auth`.
- **Restriction** : `at_rest` refusé si `stealth.role ∈ {bridge,
  gateway}` — un pont doit rebooter sans surveillance (fail-closed,
  même discipline qu'ADR-0017). Usage réaliste : poste client en zone
  à risque de saisie.
- Aucune clé dérivée en clair sur disque en mode at-rest :
  `ipv8_keypair.bin` n'existe pas (dérivation en mémoire à l'unlock).
- Lockout : mot de passe perdu → restore par phrase BIP39 → nouveau
  mot de passe. La phrase reste la recovery ultime — c'est elle qui
  rend le chiffrement acceptable.

## Conséquences

- Portabilité immédiate sans compte, sans serveur, sans friction au
  premier lancement.
- Quiconque détient blob `OBID` + mot de passe **est** l'identité —
  l'UI l'explicite ; un mot de passe faible reste brute-forceable malgré
  argon2id (la phrase BIP39 restera la voie haute entropie).
- Le fichier local en clair reste le point faible connu : un appareil
  compromis expose l'identité (comme tout wallet « hot »). Mitigation :
  permissions `0600`, pas de réplication ailleurs.
- `restore` valide le format avant d'écrire : un blob mal formé ou un
  mauvais mot de passe ne touche jamais le fichier existant.
- La phrase BIP39 = l'identité complète en clair sur papier (modèle
  wallet) : plus simple à sauvegarder, plus simple à photographier —
  le risque change de forme, pas d'ordre de grandeur.
- Le détenteur d'`api.key` peut extraire la phrase (cohérent : il
  peut déjà exporter l'identité via `OBID`).
- Le mode invité échange la continuité (confiance, contacts,
  historique) contre l'inlinkabilité inter-sessions : un observateur
  ne peut pas savoir que deux sessions invité viennent du même
  appareil — au prix d'être un inconnu permanent.
- Le mode locked ajoute un état de session inédit (API up, identité
  absente) — surface DoS locale bornée par le rate-limit d'`unlock`
  et argon2id. Le même état sert le gate de premier boot
  (`identity_pending`) : aucune clé jetable ne touche jamais le fil.
