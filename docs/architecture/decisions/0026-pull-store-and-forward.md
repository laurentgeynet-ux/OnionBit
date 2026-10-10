# ADR-0026 — Pull / store-and-forward : coffre répliqué, messagerie offline, backfill d'attestations

Statut : Proposée (2026-10-11).

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

Nouvelle table `pull_store` (créée uniquement lorsque le rôle
annonce `CAP_PULL_STORE`) :

```text
slot_key    BLOB PRIMARY KEY   -- H(recipient_pk) ou H(vault_id)
kind        INTEGER            -- encap_kind du dépôt
blob        BLOB               -- payload opaque (déjà chiffré e2e)
stored_at   INTEGER            -- epoch s
expires_at  INTEGER            -- stored_at + ttl (défaut 7 j)
```

- **Bornes** (config, jamais en dur) : `pull_store_max_per_slot`
  (dépôts par `slot_key`), `pull_store_max_total` (globale),
  `pull_store_ttl_secs`, `pull_blob_max` (≤ `ENCAP` payload).
  Éviction : expiration puis FIFO hors plus récent par slot.
- **Le pont est aveugle** : il stocke des octets ; seul le
  destinataire (ou le détenteur de la phrase) peut déchiffrer.

### 2. Kinds `encap` v2

| kind | sens | payload requête | payload réponse |
|------|------|-----------------|-----------------|
| `MAILBOX_PUT=3` | client → pont | `{slot_key, blob, ttl_hint}` | ack `MAILBOX_RESP` |
| `MAILBOX_PULL=4` | client → pont | `{recipient_pk, sig}` | `MAILBOX_RESP` : liste de blobs, puis **suppression** |
| `VAULT_PUT=5` | client → pont | `{slot_key, blob}` | ack `VAULT_RESP` (remplace l'existant) |
| `VAULT_GET=6` | client → pont | `{slot_key, auth}` | `VAULT_RESP` : blob ou `not_found` |
| `ATTEST_REQ=7` | client → pont | `{kind, subject}` | `ATTEST_RESP=8` : attestations `by_subject` bornées |

- `MAILBOX_PULL`/`VAULT_GET` portent une **signature Ed25519**
  `{kind, req_id, slot_key}` par la clé concernée — le pont ne
  livre un slot qu'à son propriétaire cryptographique (un relais
  curieux ne peut pas énumérer les slots d'autrui).
- `MAILBOX_PULL` **consomme** : les blobs rendus sont supprimés —
  le pull est le drain naturel, la TTL le filet de sécurité.
- `req_id` réutilise le corrélateur existant ; réponses à
  `req_id` inconnu droppées (comportement déjà en place).

### 3. Câblage des trois usages

- **Messagerie** : `send` sans circuit → option `deliver_offline`
  (config, défaut actif si ponts relais disponibles) : la trame
  e2e est encapsulée dans `MAILBOX_PUT` vers un pont tiré
  déterministement par `slot_key = H(dest_pk)` ; le destinataire
  exécute `MAILBOX_PULL` périodique (intervalle config, jitter —
  même discipline que `channel_sync`) au démarrage puis à chaque
  session. Réception → pipeline `on_gmsg`/`on_obf` existant,
  sans chemin parallèle.
- **Coffre** : export identité+contacts sérialisé puis chiffré
  AEAD selon la convention `OBV1` (magic+version+sel+nonce+AEAD,
  borne à l'ouverture) ; `slot_key = H("vault"|pubkey)`.
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

- `encap_kind` passe en v2 : kinds 3-8. Les kinds inconnus sont
  déjà droppés → déploiement progressif sans flag day.
- `encap_relay` ne suffit plus : le pont exécute le pull contre
  son `pull_store` local en plus du provider de découverte.
- Aucune exposition REST nouvelle en v1 pour le coffre (commande
  CLI/daemon `identity vault push/restore` à décider à
  l'implantation) ; messagerie et attestations internes.
- Dépend du P0 « gateway déployé » : les ponts actuels partagent
  le même VPS — la réplication N réplique sur le même opérateur
  tant que la diversité d'hébergeur n'existe pas (ADR-0024 §8).
