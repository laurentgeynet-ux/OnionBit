# ADR-0026 — Pull / store-and-forward : coffre répliqué, messagerie offline, backfill d'attestations

Statut : Acceptée (2026-10-11 — étapes 102-107 livrées).

## Contexte

Trois besoins distincts partagent la même primitive manquante —
« un pair hors ligne peut déposer un blob chiffré chez un relais et
le récupérer plus tard » :

1. **Messagerie offline** : `send` sans circuit vers le destinataire
   échoue aujourd'hui (`failed` en historique, oracle online-only).
   Les trames `GMSG`/`OBF` sont **déjà chiffrées de bout en bout** —
   un relais qui les conserve est aveugle sur le contenu.
2. **Coffre d'identité répliqué** : la phrase BIP39 permet de
   restaurer la clé, mais pas les contacts ni les conversations —
   il faut un export chiffré (convention `OBV1`/`OBF`) répliqué,
   récupérable sur n'importe quel appareil.
3. **Backfill d'attestations** : `AttestationStore` est local ;
   un nœud récent (ou resynchronisé) ne connaît que les
   attestations entendues en direct depuis sa connexion.

L'étape 100 (ADR-0025 §5) a livré le transport : la trame `ENCAP`
générique (`{v, kind, req_id, varlen(payload)}`, signée, bornée à
2048 o, relais `bridge`/`gateway` via `CAP_DISCOVERY_RELAY`, budget
par clé signataire). Cette ADR étend `encap_kind` et ajoute le
store — **un seul mécanisme, trois usages**.

## Décision

### 1. Store chiffré borné sur les ponts

Nouvelle table `pull_store` (migration v24) :

```text
seq         INTEGER PRIMARY KEY AUTOINCREMENT  -- ordre FIFO
slot        BLOB NOT NULL      -- sha256(domaine‖pk), 32 o
kind        INTEGER NOT NULL   -- 1 mailbox, 2 vault
blob        BLOB NOT NULL      -- payload opaque (déjà chiffré e2e)
stored_at   INTEGER NOT NULL   -- epoch s
expires_at  INTEGER NOT NULL   -- stored_at + ttl (défaut 7 j)
```

`slot` n'est **pas** une clé primaire : la boîte aux lettres
empile plusieurs dépôts par slot (FIFO), le coffre n'en garde
qu'un (`put` remplace). L'index de slot n'est jamais transporté
sur le fil — il est **dérivé serveur** de la clé prouvée :
`pull_slot("obmbox:"‖pk)` / `pull_slot("obvault:"‖pk)`.

- **Bornes** (`PullStoreConfig`, jamais en dur) :
  `max_per_slot` (64), `max_total` (65 536), `ttl_secs` (7 j),
  `blob_max` (1 800 o ≤ `ENCAP` payload), `pull_limit` (32).
  Éviction : expiration puis FIFO par slot puis globale.
- **Le pont est aveugle** : il stocke des octets ; seul le
  destinataire (ou le détenteur de la phrase) peut déchiffrer.

### 2. Kinds `encap` v2

| kind | sens | payload requête | payload réponse |
|------|------|-----------------|-----------------|
| `MAILBOX_PUT` | client → pont | `{slot:32, varlen blob}` | `MAILBOX_RESP` : ack `{0/1}` |
| `MAILBOX_PULL` | client → pont | `{varlen pk, sig:64}` | `MAILBOX_RESP` : blobs, puis **suppression** |
| `VAULT_PUT` | client → pont | `{varlen pk, sig:64, varlen blob}` | `VAULT_RESP` : ack (remplace) |
| `VAULT_GET` | client → pont | `{varlen pk, sig:64}` | `VAULT_RESP` : blob ou terminateur vide |
| `ATTEST_REQ` | client → pont | `{kind:u8, varlen subject}` | `ATTEST_RESP` : attestations `by_subject` bornées |

Réponses : une trame `*_RESP` par élément, payload
`{more:u8, varlen data}` — terminateur vide = slot vide /
`not_found`.

- `MAILBOX_PULL`/`VAULT_GET`/`VAULT_PUT` portent une **signature
  Ed25519** `{kind, req_id, pk}` (`pull_auth_msg`) par la clé
  concernée — `pk` est la `LibNaClPK` sérialisée prouvée, le
  `slot` est dérivé serveur : un relais curieux ne peut pas
  énumérer les slots d'autrui (le hash n'est pas dans le signé,
  il *est* déterminé par le signataire). `VAULT_PUT` est signée :
  sans elle, n'importe qui pourrait écraser le coffre d'autrui.
- `MAILBOX_PUT` est **anonyme par design** : n'importe qui peut
  déposer dans une boîte (c'est le principe d'une boîte aux
  lettres), seul le propriétaire peut retirer.
- `MAILBOX_PULL` **consomme** : les blobs rendus sont supprimés —
  le pull est le drain naturel, la TTL le filet de sécurité.
- `req_id` réutilise le corrélateur existant ; la signature le
  couvre → pas de rejeu d'une auth sur une autre requête.

### 3. Câblage des trois usages

- **Messagerie** : `send` sans circuit → `deliver_offline`
  (`MessagingConfig`, défaut actif ; clés daemon
  `messaging_deliver_offline` + `messaging_offline_poll_secs`) :
  la trame `msg` **filaire identique** (AEAD + signature
  Ed25519) est scellée avec une `send_key` dérivée du **DH
  statique de paire** (`crypt_pk` destinataire × `crypt_sk`
  émetteur — module `obox`), encapsulée `{f: wire, p: pk}` dans
  `MAILBOX_PUT` vers tous les ponts `CAP_PULL_STORE` connus ;
  le destinataire exécute `MAILBOX_PULL` périodique jitter au
  tick de maintenance. Réception → `ingest_offline` : même
  `Frame::open` (vérification de signature inchangée),
  consentement `Active` requis, dedup `id`, livraison
  `received` — sans chemin parallèle de codec. Les clés de
  **circuit** ne pouvaient pas être réutilisées (elles dérivent
  du secret e2e du lien) : le DH statique de paire est la
  solution — la trame reste la même, seule la dérivation
  change.
- **Coffre** : blob `export_vault()` existant (convention
  `OBV1` : magic+version, `pair_seal_in` pour soi-même, borne à
  l'ouverture) ; `slot = pull_slot("obvault:", pk)`.
  `VAULT_PUT` sur **tous** les ponts annonçant `CAP_PULL_STORE`
  (réplication N = nb de ponts, pas de consensus), `VAULT_GET`
  à la restauration. `VAULT_PUT` remplace : le coffre n'est pas
  une boîte aux lettres, un seul état courant.
- **Attestations** : `ATTEST_REQ` vers les ponts à la connexion
  (backfill borné — `max_attest_per_subject`), ingestion par le
  pipeline `attest` existant (dedup `(curator,kind,subject)`,
  vérification de signature identique — une attestation relayée
  n'a pas moins de preuve qu'une attestée en direct).

### 4. Capacité et rôles

Nouveau bit `CAP_PULL_STORE` (bit 4 de `hello.caps`), annoncé par
les rôles `bridge`/`gateway` quand `pull_store_enabled` (défaut
actif pour ces rôles — c'est leur fonction). Les clients choisissent
parmi `relay_peers()` filtrés sur la capacité ; les ponts sans la
capacité ignorent les kinds inconnus (déjà le comportement).

### 5. Menaces et budgets

- **Amplification/remplissage** : le budget `encap` par clé
  signataire borne le débit ; les bornes par slot + totale bornent
  l'espace ; `MAILBOX_PUT` pour un `slot_key` arbitraire est
  inoffensif (le pont est payé en octets bornés, le destinataire
  fictif ne lit jamais).
- **Énumération** : `VAULT_GET`/`MAILBOX_PULL` exigent la
  signature du propriétaire du slot — pas d'énumération.
- **Pont voyant les métadonnées** : il voit `slot_key` (hash, pas
  la clé) + timing — même compromis VPN-like documenté en
  ADR-0025 §5 (confiance vis-à-vis du pont, anonymat vis-à-vis
  des pairs) ; la réplication N réduit la dépendance, pas la
  visibilité.
- **Livraison garantie ?** Non — best-effort borné par TTL. Un
  accusé applicatif (`GCTL` reçu quand le pair revient en ligne)
  reste le signal de livraison réelle.

## Alternatives écartées

- **Gossip/mesh des blobs** : rejeté — Tribler l'a abandonné pour
  les métadonnées (volume), même raison ici ; le pull borné
  suffit.
- **Store-and-forward distribué à la Bitmessage** : rejeté —
  coût de routage/anti-spam disproportionné ; nos ponts sont des
  points de confiance assumés (déjà le cas des circuits).
- **Un mécanisme par usage** : rejeté — la trame `ENCAP` a été
  spécifiée générique précisément pour ça.

## Conséquences

- `encap_kind` passe en v2 : kinds 3-10 (`MAILBOX_PUT=3`,
  `MAILBOX_PULL=4`, `MAILBOX_RESP=5`, `VAULT_PUT=6`,
  `VAULT_GET=7`, `VAULT_RESP=8`, `ATTEST_REQ=9`,
  `ATTEST_RESP=10`). Les kinds inconnus sont déjà droppés →
  déploiement progressif sans flag day.
- `encap_relay` ne suffit plus : le pont exécute le pull contre
  son `pull_store` local (trait `PullStoreBackend`, adaptateur
  `DbPullStore` SQLite) en plus du provider de découverte, sous
  `pull_store_relay` (rôles `bridge`/`gateway`).
- Surface REST du coffre (tranchée à l'implantation) :
  `POST /api/messaging/vault/replicate` (push multi-ponts,
  `{replicated}`) et `POST /api/messaging/vault/restore`
  (`VAULT_GET` + import, `{restored}`) — même périmètre
  `api_key_auth` que le reste de la messagerie.
- Le backfill `ATTEST_REQ` part au démarrage pour `IDENTITY`/
  propre clé (retardé — les `caps` des ponts doivent être
  connues) ; ingestion `ingest_backfilled_attestation` = pipeline
  `attest` sans re-gossip.
- Dépend du P0 « gateway déployé » : les ponts actuels partagent
  le même VPS — la réplication N réplique sur le même opérateur
  tant que la diversité d'hébergeur n'existe pas (ADR-0024 §8).
