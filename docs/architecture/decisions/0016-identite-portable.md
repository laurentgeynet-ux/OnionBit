# ADR-0016 — Identité portable : export/import chiffré (solution intérimaire)

Statut : **partiellement implémentée** (2026-10-06) — l'export/import
`OBID` et le durcissement du fichier local sont livrés ; la graine
BIP39/phrase de récupération reste une décision ouverte.

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

## Différé (décision ouverte)

L'architecture cible reste à choisir — probablement une graine
`identity_seed.bin` (32 o) racine de dérivation HKDF, encodée en phrase
BIP39 24 mots, le fichier de clé devenant un cache régénérable. Aussi
ouvert : chiffrement « at rest » optionnel du fichier local (DPAPI /
trousseau OS / mot de passe) et multi-profils (un `state_dir` par
identité, sélecteur au démarrage).

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
