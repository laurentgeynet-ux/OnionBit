# ADR-0019 — Messagerie : conversations multiples, groupes et pièces jointes

Statut : **Proposée** (2026-10-08, revue 2026-10-09 — alignement sur
ADR-0018 implémentée) — extension de la messagerie e2e
d'ADR-0011 (livrée, Phase 8, bancs `MS-*` verts). Réutilise sans les
modifier : les circuits e2e et le hidden seeding
(`onionbit-tunnel`), le codec et l'anti-replay
(`onionbit-messaging`), les capacités `hello.caps` d'ADR-0015
(`CAP_MSG_V1` = bit 1), et **ADR-0018 Acceptée** (étapes 57–63) :
racines portables `@state`/`@public`/`@private`
(`onionbit-core::paths`), `StorageArea`/`storage.default_area`,
grammaire `destination` `"<chemin>" | {area, dir?}`, zone privée
`OBD`/`OBM` (`PrivateZone`, `store_root`, état
`locked|mounted|guest`), `downloads.storage_area` (v20),
`GET /api/private`, `storage_removable`/`identity.at_rest` et
sélecteur de zone UI (privée grisée si `locked`). Cadre de la
Phase 12 de `docs/plans/roadmap.md`.

## Contexte

La messagerie actuelle est strictement **1:1** : un contact = une
clé publique = un swarm de présence `messaging_hash(pk)` = un
circuit e2e à la fois. L'UI Flutter n'ouvre qu'une conversation à
la fois (colonne contacts + panneau unique, `messaging_page.dart`).

Trois demandes convergentes :

1. **Onglets multiples** — parler à des correspondants différents
   en parallèle, chaque conversation dans son onglet ;
2. **Chat groupé** — une conversation commune à plusieurs membres ;
3. **Pièces jointes** — import de fichier et glisser-déposer dans
   la zone de messagerie, envoyé en pièce jointe au correspondant
   ou à tous les membres d'un groupe.

Constats structurants, vérifiés contre le code :

- **Le transport est pair-à-pair.** Un circuit e2e lie exactement
  deux démons : le swarm cible porte `messaging_hash(pk)` du
  destinataire, `link-e2e` pose des `hs_session_keys` à deux. Il
  n'existe **pas** de canal multicast dans le tunnel : un groupe
  est nécessairement un assemblage de liaisons 1:1.
- **La trame est petite et rare par construction.** `body` ≤
  30 Kio, seau ~2 trames/s/contact (`MessagingConfig`) : y faire
  passer des fichiers demanderait découpage, reprise et contrôle
  de flux — réinventer BitTorrent mal, sous des budgets pensés
  pour du texte, en concurrence avec lui.
- **La machinerie de transfert existe déjà.** `POST /api/
  createtorrent` (librqbit vendored), seeding anonyme
  (`join_swarm(ih, hops, seeding=true)` → points d'introduction
  `IP_SEEDER` + annonce `DHTIntroPointPayload`), résolution magnet
  via `ut_metadata` sur le flux de pairs du swarm caché
  (`peer_info_reader`, flux de pairs dynamique du moteur), et
  téléchargement anonyme complet (`anon_hops`, `safe_seeding`
  obligatoire). Une pièce jointe est un téléchargement à un
  seed — le pipeline éprouvé fait tout le travail.
- **L'UI sait déjà glisser-déposer** : `DropZone` global filtre
  `.torrent`/`.magnet` vers `pendingFilesProvider` →
  `AddDownloadDialog`. Une zone scopée à la conversation suffit.
- **L'UI peut être distante** (web same-origin, pilotage mobile) :
  le daemon ne lit pas le FS du client — le fichier monte en
  **upload** vers le daemon avant tout envoi.

## Décision

### 1. « Conversation » : abstraction commune aux trois demandes

Une **conversation** (`conv_id`, 16 octets) est l'unité
d'adressage applicatif — les onglets UI, les groupes et les pièces
jointes s'y rattachent sans exception :

- **conversation directe** : `conv_id` **déterministe** —
  `SHA1("onionbit/conv/direct/v1" ‖ min(pk_a,pk_b) ‖
  max(pk_a,pk_b))[:16]`. Calculable des deux côtés sans
  négociation ; les trames v1 existantes (sans `conv`) s'y
  rattachent par la `pk` vérifiée de l'émetteur — la migration des
  historiques est mécanique ;
- **conversation de groupe** : `conv_id` aléatoire 128 bits tiré
  par le créateur, transmis dans l'invitation — jamais dérivable
  des membres, donc non corrélable hors du groupe.

Les onglets sont une affaire **purement UI** : la colonne gauche
devient une liste de conversations (contacts directs + groupes +
invitations), le panneau droit un ensemble d'onglets ouverts —
aucun impact filaire, aucune nouvelle entité réseau. L'état
« non lu » est porté par `last_read_ts` par conversation en base
et rafraîchi par le flux SSE existant.

### 2. Trame v2 : champ `conv` + capacités

La trame v1 `{v, type, id, seq, ts, body, sig}` est étendue :

```text
v2 = { "v":2, "conv": <16 octets>, "type": <…>, "id": <16>,
       "seq": <u64>, "ts": <u64>, "body": <chiffré>, "sig": <64> }
```

- même discipline que v1 : dictionnaire canonique strict (clé
  `conv` obligatoire en v2, ensemble de clés exact), bornes
  `max_frame_len`/`max_body_len` inchangées, signature Ed25519 sur
  la forme canonique (le `conv` entre donc **sous la signature** —
  une trame de groupe ne peut pas être re-étiquetée vers un autre
  groupe), `seq`/`id`/fenêtre par lien pair-à-pair inchangés ;
- **types étendus** : `hello`/`accept`/`reject`/`msg`/`ack`
  inchangés + **`gctl`** (contrôle de groupe : invitation,
  adhésion, départ, roster — `body` = bencode strict) et
  **`attach`** (offre de pièce jointe — `body` = descripteur
  bencode strict) ;
- **`hello`/`accept` v2** : `body = pk_bin(74) ‖ caps:u64be` —
  annonce in-band des capacités ; bit 0 = `WANT_GROUPS` (l'invite
  d'un pair qui a `groups_enabled=false` est refusée avant envoi,
  jamais reçue). Les autres `body` restent applicatifs ;
- **découverte pré-contact** : `CAP_MSG_V2 = 1<<2` dans les
  `hello.caps` de la communauté ext — même pattern que
  `CAP_MSG_V1` (bit 1), annoncé seulement si la messagerie tourne
  réellement. Sans ext, à la liaison le pair v2 envoie son `hello`
  v1 **puis** un `hello` v2 (un pair v1 le rejette proprement au
  préfiltre `v != 1` → `UnknownVersion`, coût = une trame
  écartée) — le pair v2 enregistre « parle v2 » à la réception ;
- **compatibilité** : le texte direct reste émis en **v1** tant
  que la capacité v2 du pair est inconnue — les installs Phase 8
  continuent de converser sans rien voir. Un pair v1 recevant une
  trame v2 la rejette au préfiltre, sans casse ni affichage
  parasite ;
- validation du `conv` entrant : `conv` = conv directe dérivée de
  la paire, **ou** conv d'un groupe connu dont l'émetteur figure
  au roster. `gctl invite` vers un `conv` inconnu est le seul
  frame admis sur un conv inexistant — c'est la porte d'entrée du
  groupe (bornée comme `pending` : `group_pending_cap`,
  `group_pending_ttl_secs`).

### 3. Groupes — maillage pair-à-pair, portée de consentement

Pas de nouveau transport : un groupe est une **conversation
multi-membres greffée sur les liaisons e2e 1:1 existantes**.

- **Membres** : `msg_members(conv_id, member_pk, added_by, state,
  joined_at)`. Le roster se propage par `gctl` : `invite` {conv,
  name, roster, by}, `join` (acceptation du membre), `leave`,
  `roster` (synchronisation complète à la jointure et à chaque
  changement — dernier écrivain gagne sur `(joined_at, pk)`).
- **Portée de consentement — point de sécurité central** : un
  membre du roster qui n'est *pas* un de mes contacts obtient une
  portée **strictement confinée au groupe** : ses trames ne sont
  admises que si `conv` ∈ mes groupes ∧ `pk` ∈ roster de ce conv.
  Il ne devient pas contact, son `hello` ne crée **pas** de
  `pending` (un pair déjà `scope='group'` est filtré avant la
  machine de consentement), il ne peut pas m'écrire en direct.
  Promotion en contact = action utilisateur explicite. À
  l'inverse, un membre qui est déjà contact garde sa portée
  complète — même table, même seq.
- **Consentement de groupe** : l'invitation ne peut arriver que
  sur un lien déjà consenti (un inconnu ne peut pas lier de
  circuit — ADR-0011) ; elle ouvre une entrée `invited` bornée →
  l'utilisateur accepte (`join` émis aux membres joignables) ou
  décline (`leave` à l'invitant, conv oubliée). Membership ouvert
  — tout membre peut inviter ; pas d'admin ni d'exclusion en v1
  (limite assumée, §8).
- **Émission = fan-out** : un message de groupe est scellé une
  fois logiquement (`mid` applicatif 16 octets + `author` =
  signataire, dans le `body`) puis envoyé en une trame v2
  `conv=G` **par membre lié**. `seq` et `ack` restent par lien —
  l'état de livraison exposé est par membre (`msg_delivery` :
  `sent|acked|failed` — « livré à 3/5 » en UI). Membre hors ligne
  → `failed` pour lui seul, pas de file (online-only hérité).
- **Liaison aux membres** : le client joint le swarm de présence
  de chaque membre (`join_swarm(messaging_hash(pk_m))` — déjà la
  mécanique des contacts restaurés) et relie un circuit e2e ; les
  annonces DHT du groupe sont **nulles** — aucun swarm de groupe
  n'existe, seules les liaisons pair-à-pair changent (la
  métadonnée de présence est celle des membres, déjà inscrite au
  threat model).
- **Ordre et doublons** : ordre d'affichage total local
  `(ts, author, mid)` — ordre **non causal assumé** (pas
  d'horloges vectorielles en v1) ; dédup par `(conv, author, mid)`
  bornée — une réémission après réouverture de circuit retombe
  dans la dédup, jamais dans l'historique.
- **Bornes** : `group_max_members` (défaut 16 — chaque membre =
  un circuit e2e = `hops+1` circuits tunnel), `group_max_convs`,
  `group_pending_cap`/`_ttl_secs`. Les budgets par contact et
  global couvrent les trames de groupe sans changement — un flood
  de groupe est un flood de liens, déjà borné.

### 4. Pièces jointes — torrent éphémère dans le flux message

Une pièce jointe n'est **jamais** dans la trame : la trame porte
un descripteur magnet, le contenu circule par le pipeline
BitTorrent anonyme existant (intégrité par infohash, reprise,
débit tunnel complet — aucun budget de trames engagé).

```text
émetteur                                    récepteur
upload ──► staging @state ──► createtorrent   attach {ih,name,size}
              │               salé + seed     ──► bulle « Recevoir »
              │               join_swarm(ih)        │ accept explicite
              └──────────────────────────────────►│ magnet + anon_hops
                          swarm caché (ih)          └─► download vérifié
```

- **Upload** : `POST /api/messaging/uploads` (octets bruts,
  `?name=`) → `@state/messaging/uploads/<upload_id>` ; borné
  par fichier (`attach_max_mib`) et globalement
  (`attach_stage_max_mib`), TTL `upload_ttl_secs` — un upload
  jamais attaché est purgé au tick. Nécessaire pour l'UI web et
  le pilotage mobile ; l'app desktop emprunte le même chemin (un
  seul flux, uniforme). Variante locale : `uploads` accepte aussi
  `{path: "@…/…"}` d'un fichier déjà sous `@public` ou hors
  racines (zéro copie pour les fichiers déjà dans `data/public`) —
  **`@private` est exclu** de cette variante (`files/browse` le
  refuse déjà en 403) ; attacher un fichier privé passe par
  lecture via `TorrentStorage` (voir « posture zone privée »).
- **Émission** : `POST …/conversations/{conv}/attachments
  {upload_ids:[…], note?}` — chaque upload est déplacé sous
  `@state/messaging/attachments/<attach_id>/`, puis
  `librqbit::create_torrent` avec **salage entropique** du dict
  `info` (champ `x-onionbit` = 16 octets aléatoires — rend
  l'infohash indévinable : un hash de contenu connu pointerait le
  swarm ; le nom réel reste propre, le sel ne sort pas du torrent).
  `CreateTorrentOptions` n'expose aujourd'hui que
  `name/trackers/piece_length` → **micro-patch vendored** (lignée
  ADR-0007, ~option `info_extra`) — alternative sans patch
  rejetée : saler le `name` polluerait le nom du fichier reçu.
  Le torrent est ajouté à la session en **seed** avec
  `anon_hops` = sauts messagerie, `output_folder` = le dossier de
  staging (aucune copie de données), `origin = "messaging"` en
  table `downloads` (survit au restart via `restore_downloads` —
  une offre reste servie après reboot), et
  `join_swarm(ih, hops, seeding=true)` annonce les points
  d'introduction sous `ih`. La trame `attach`
  `{files:[{ih,name,size}], note?}` est émise en fan-out aux
  membres liés (`attach_max_per_msg`).
- **Réception** : la trame crée une entrée `offered` — **jamais
  de téléchargement automatique** (posture de consentement
  d'ADR-0011 étendue au contenu). Le clic « Recevoir » appelle
  `POST /api/messaging/attachments/{aid}/accept` → ajout
  `magnet:?xt=urn:btih:<ih>` avec `anon_hops` et la **grammaire
  `destination` d'ADR-0018 réutilisée telle quelle** : zone =
  `attach_area` (défaut `public`, surchargeable par requête
  `{area:"private"}` — même `resolve_area`, même
  `409 identity_locked` tant que la zone est fermée). Sous
  `public`, sous-dossier dédié `@public/messaging/` ; sous
  `private`, `destination:{area:"private"}` sans `dir` — la zone
  ne connaît que `temp`/`downloads` et les noms y sont opaques
  par construction. Les métadonnées arrivent par `ut_metadata`
  sur le swarm caché (le seed initial est l'émetteur seul) ; un
  privé reçu prend `move_on_completion` `temp→downloads` comme
  tout privé. `decline` oublie l'offre ; l'expiration du
  seed émetteur bascule l'offre en `expired` au timeout des
  métadonnées — échec borné et visible, comme l'offline.
- **Groupe** : le même `ih` sert tous les membres — un seul
  upload pour N correspondants ; chaque membre ayant téléchargé
  peut re-seeder via le même swarm caché (opportuniste — la
  croix se fait par les points d'introduction, sans travail
  dédié).
- **Cycle de vie émetteur** : `attach_seed_ttl_secs` (défaut
  7 jours, `0` = politique `seeding_mode` normale) → expiration =
  `leave_swarm` + arrêt du download ; `attach_purge_on_expire`
  purge aussi le fichier stagé. `remove_data` de l'offre
  supprime le fichier sous `@state`.
- **Posture zone privée** (ADR-0018) : attacher un fichier de
  `@private` passe par lecture via `TorrentStorage` → copie **en
  clair** dans le staging `@state/` — flux assumé et documenté
  (le staging vit dans `state/`, zone sensible par construction,
  jamais sous `data/public`). En session **invitée**, une pièce
  jointe reçue en `private` vit sous `temp/.guest/` et est purgée
  à la fermeture — cohérent avec « rien n'est conservé ».
- **Métadonnées en clair — limite de périmètre assumée** : pour
  un attach **reçu en zone privée**, le contenu est protégé
  (`.obd` + catalogue `manifest.obm`) mais la ligne
  `msg_attachments` et le corps du message (`{ih, name, size}`)
  restent en clair dans `onionbit.db` — persistance en clair v1
  assumée d'ADR-0011. La zone privée couvre le **contenu**, pas
  les métadonnées du fil de discussion ; la ligne `downloads`
  reste opaque (`HMAC(infohash)`), cohérent avec ADR-0018.
- **Intégrité** : l'infohash porte l'authenticité du contenu —
  aucun relais ni membre ne peut altérer le fichier servi (hash
  de pièce vérifié par le moteur). La confidentialité du contenu
  = celle des circuits e2e + l'indévinabilité du swarm.

### 5. Persistance — nouvelle migration (après v20)

v20 (ADR-0018) a déjà posé `downloads.storage_area` ; cette
migration ajoute les tables de messagerie et la colonne
`downloads.origin` (`'user'` défaut, `'messaging'` pour les
offres seedées — filtrable dans l'UI Téléchargements).

```sql
ALTER TABLE msg_contacts ADD COLUMN scope TEXT NOT NULL
    DEFAULT 'contact' CHECK (scope IN ('contact','group'));
    -- 'group' = pair connu seulement via un roster : jamais dans
    -- la liste de contacts, jamais de pending automatique.
CREATE TABLE msg_conversations (
    conv_id      BLOB PRIMARY KEY,     -- 16 o
    kind         TEXT NOT NULL CHECK (kind IN ('direct','group')),
    name         TEXT NOT NULL DEFAULT '',
    state        TEXT NOT NULL DEFAULT 'active'
                 CHECK (state IN ('invited','active','left')),
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    last_read_ts INTEGER NOT NULL DEFAULT 0   -- badge non lu
);
CREATE TABLE msg_members (
    conv_id    BLOB NOT NULL REFERENCES msg_conversations(conv_id)
               ON DELETE CASCADE,
    member_pk  BLOB NOT NULL,          -- ≠ ligne contact (scope)
    added_by   BLOB NOT NULL,
    state      TEXT NOT NULL CHECK (state IN ('member','left')),
    joined_at  INTEGER NOT NULL,
    PRIMARY KEY (conv_id, member_pk)
);
ALTER TABLE msg_messages ADD COLUMN conv_id BLOB;
ALTER TABLE msg_messages ADD COLUMN author_pk BLOB;  -- auteur groupe
ALTER TABLE msg_messages ADD COLUMN mid BLOB;        -- dédup groupe
CREATE TABLE msg_delivery (
    msg_id    BLOB NOT NULL REFERENCES msg_messages(id)
              ON DELETE CASCADE,
    member_pk BLOB NOT NULL,
    status    TEXT NOT NULL CHECK (status IN ('sent','acked','failed')),
    ts        INTEGER NOT NULL,
    PRIMARY KEY (msg_id, member_pk)
);
CREATE TABLE msg_attachments (
    attach_id  BLOB PRIMARY KEY,
    conv_id    BLOB NOT NULL,
    msg_id     BLOB NOT NULL,
    ih         BLOB NOT NULL,          -- infohash salé
    name       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    role       TEXT NOT NULL CHECK (role IN ('offer','recv')),
    state      TEXT NOT NULL,          -- seeding|offered|accepted|
                                       -- downloading|done|expired|
                                       -- declined
    created_at INTEGER NOT NULL
);
ALTER TABLE downloads ADD COLUMN origin TEXT NOT NULL
    DEFAULT 'user' CHECK (origin IN ('user','messaging'));
    -- 'messaging' = swarm caché offert par la messagerie (seed ou
    -- réception) ; filtrable côté UI Téléchargements.
```

Migration des données : chaque ligne `msg_messages` reçoit la
`conv_id` directe dérivée de son `contact_pk` ; la conv directe de
chaque contact est matérialisée à la volée. Suppression réelle
conservée (`DELETE`, cascades) ; la rétention `retention_secs`
s'applique par conversation (un contact = sa conv directe).

### 6. API REST, SSE et UI

Section `/api/messaging/*` étendue (extension Rust documentée
dans `api_rest_mapping.md`, mêmes gardes : 404 messagerie off,
`api_key_auth`) :

| Route | Rôle |
| :--- | :--- |
| `GET /conversations` | liste `{conv_id, kind, name, members, last_ts, unread, link}` |
| `POST /groups` `{name, members:[pk…]}` | créer + inviter (membres ⊂ contacts actifs) |
| `GET/POST /conversations/{conv}/messages` | historique borné / envoi |
| `POST /conversations/{conv}/read` | marqueur de lecture (badge) |
| `POST /conversations/{conv}/invite` `{public_key}` | inviter un membre |
| `POST /conversations/{conv}/leave` | quitter le groupe |
| `POST /uploads` `?name=` / `DELETE /uploads/{id}` | staging de fichiers |
| `POST /conversations/{conv}/attachments` `{upload_ids, note?}` | émission attach |
| `GET /attachments/{aid}` + `POST …/accept {destination?}` / `…/decline` | état + décision (`destination` = grammaire ADR-0018 `{area,dir?}`, défaut `attach_area`) |

SSE : `messaging_frame` gagne `conv` ; nouveaux topics
`messaging_conv` (created/invited/updated/left) et
`messaging_attach` (offered/state/progress). Compatibilité :
`GET/POST /contacts/{pk}/messages` demeurent — alias de la conv
directe.

UI Flutter (`features/messaging`) : liste de conversations avec
badges non lus et invitations de groupe actionnables ; onglets de
conversations ouvertes (état local, `ui_prefs`) ; dialogue de
création de groupe (multi-sélection de contacts actifs) ; panneau
membres ; **zone de glisser-déposer scopée à la conversation**
(tout fichier → pipeline upload→attach ; la `DropZone` globale
`.torrent`/`.magnet` reste sur le reste de la fenêtre) ; bulle
pièce jointe (nom, taille, progression mappée du download,
états `offered/done/expired`) ; boutons groupe/fichier grisés
avec infobulle quand le pair n'annonce pas v2. Côté zones :
l'option « recevoir en privé » n'est offerte que si
`GET /api/private` annonce `mounted` (ou `guest`, marqué
éphémère) ; les chemins affichés passent par
`display_stored_path` — l'UI ne calcule jamais de chemin absolu
(portable ADR-0018).

### 7. Configuration (aucune valeur en dur)

```jsonc
{
  "messaging": {
    "groups_enabled": true,          // annonce WANT_GROUPS + accepte invite
    "group_max_members": 16,         // circuits e2e par nœud ≈ membres-1
    "group_max_convs": 64,
    "group_pending_cap": 16,
    "group_pending_ttl_secs": 3600,
    "attach_max_mib": 512,           // borne par fichier uploadé
    "attach_max_per_msg": 8,
    "attach_stage_max_mib": 4096,    // quota global du staging
    "attach_seed_ttl_secs": 604800,  // 0 = politique seeding normale
    "attach_area": "public",         // zone de réception ADR-0018 ;
                                     // "private" → 409 tant que la
                                     // zone est fermée ; "guest" =
                                     // éphémère, purgé à la fermeture
    "upload_ttl_secs": 86400
  }
}
```

### 8. Menace et limites assumées

| Menace | Mitigation | Limite honnête |
| :--- | :--- | :--- |
| Spam de membres inconnus via un groupe | portée `group` confinée au conv — pas de contact, pas de pending, pas de 1:1 | un membre consenti peut introduire des tiers : **membership ouvert**, ni admin ni kick en v1 (le départ local reste la sortie) |
| Empreinte réseau du groupe | aucun swarm de groupe — pas d'annonce DHT supplémentaire ; seules des liaisons e2e de plus | le nombre de circuits e2e par nœud croît (N-1) : volume observable par les relais, borné par `group_max_members` |
| Pièce jointe : swarm observable | infohash **salé** (`x-onionbit` dans `info`) → non dévinable, non corrélable au contenu | taille du fichier visible dans le swarm ; l'offre expire avec le seed de l'émetteur (online-only hérité — pas de store-and-forward) |
| Ordre des messages de groupe | dédup `(conv,author,mid)` + ordre total local `(ts,author,mid)` | ordre **non causal** assumé — deux réponses simultanées peuvent s'afficher inversées entre membres |
| Malware | réception = clic explicite, jamais automatique | le contenu reçu est exécuté sous la responsabilité utilisateur (même posture qu'un magnet reçu) |
| Fichier privé attaché | lecture via `TorrentStorage` | copie **en clair** dans `@state/` (zone sensible assumée, purgeable) |
| Attach reçu en zone privée | contenu `.obd` + catalogue `manifest.obm`, ligne `downloads` opaque | les métadonnées du fil (`msg_attachments`, corps du message `{ih,name,size}`) restent **en clair** dans `onionbit.db` — la zone couvre le contenu, pas les métadonnées (persistance en clair v1 d'ADR-0011) |

## Alternatives rejetées

- **Chunks de fichier dans les trames** : 30 Kio × ~2 trames/s →
  ~60 Kio/s plafond, réinvention de la reprise/intégrité/contrôle
  de flux, en concurrence avec le texte — le pipeline BitTorrent
  anonyme fait déjà tout cela, éprouvé.
- **Swarm de groupe / canal multicast** : n'existe pas dans le
  modèle de circuits e2e (une liaison = deux extrémités).
- **Relais des messages par les membres** (gossip) au lieu du
  maillage complet : moins de circuits mais retenue
  indistinguable d'une coupure, réordonnancement pire, surface de
  confusion — à 16 membres le maillage complet reste simple et
  chaque trame reste directe signée.
- **Clé symétrique de groupe unique** : casserait
  l'authentification par paire (qui signe ?), l'anti-replay par
  lien et ajouterait une compromission globale — les clés restent
  par liaison, le `body` est scellé par membre.
- **Pièce jointe inline (base64 ≤ 30 Kio)** : inutile au-delà
  d'un favicon — un seul mécanisme, uniforme.
- **Envoi direct UDP/TCP hors tunnel** entre correspondants :
  fuite d'adressage frontale — jamais.
- **Kick/admin de groupe v1** : exige une autorité signée par le
  créateur (rotation, révocation) — backlog explicite plutôt
  qu'un modèle bancal.

## Conséquences

- **Positif** : les trois demandes couvertes par **un** concept
  (conversation) + un champ de trame + un descripteur magnet ;
  zéro nouveau transport, zéro nouveau codec de données ; la
  pièce jointe hérite gratuitement intégrité, reprise, débit
  tunnel et seed multi-membres ; compat v1 propre (rien ne casse
  chez les pairs Phase 8) ; aucune annonce DHT de groupe.
- **Négatif** : surface de protocole nouvelle (v2 + `gctl` +
  `attach` → fuzz et tests hostiles obligatoires) ; pression de
  circuits par nœud dans les grands groupes (bornée) ; un second
  patch vendored (`info_extra` du create_torrent) ; staging
  disque à administrer (bornes + TTL).
- **Interop** : strictement OnionBit↔OnionBit, comme la
  messagerie v1 — Tribler n'y voit rien (écart assumé et
  documenté).

## Tests attendus (Phase 12 — bancs `MG-*`, catalogue
`docs/plans/bancs_tests.md`)

- **Codec v2** : roundtrip, `conv` absent/inconnu, `v` mixte sur
  un même lien (v1 puis v2), corps `gctl`/`attach` hostiles
  (bencode strict, bornes) — corpus fuzz `messaging_frame` étendu ;
- **Conversations** : dérivation déterministe directe (même conv
  des deux côtés, collision-free), mapping des historiques v1,
  migration DB sur base réelle ;
- **Groupe loopback 3 nœuds** : création → invite → join → msg
  fan-out → roster sync → membre invité par un non-créateur →
  leave ; non-membre parlant `conv=G` → drop + compteur ;
  pair `scope='group'` : aucun `pending`, aucun 1:1 ;
- **Dédup/ordre groupe** : `mid` réémis après réouverture de
  circuit → un seul affichage ; statuts par membre (`failed`
  hors ligne, `acked` en ligne) ;
- **Pièce jointe loopback** : upload → attach → accept →
  download → **SHA-256 du contenu identique** ; deux envois du
  même fichier → deux `ih` distincts (oracle du sel) ; bornes
  upload/`attach_max_per_msg` ; expiration de seed → offre
  `expired` sans boucle ; suppression = fichier stagé retiré ;
- **Non-fuite** : aucun octet de fichier hors tunnel ; le fichier
  stagé ne sort jamais sous `data/public` (oracle listing) ; un
  `.obd` privé attaché passe bien par `TorrentStorage` ; un
  attach accepté `area=private` est catalogué `manifest.obm` et
  sa ligne `downloads` opaque (`HMAC(infohash)`) ; accept privé
  zone fermée → `409 identity_locked` ; invité → privé purgé à
  la fermeture ; la variante `uploads {path}` refuse `@private` ;
- **Caps** : `CAP_MSG_V2` annoncé seulement si messagerie +
  `groups_enabled` ; pair v1 → dégradation propre (v2 droppé au
  préfiltre, v1 continue) ;
- **Restart** : conversations, rosters, offres d'attachement et
  statuts restaurés ; l'offre `seeding` re-seed après reboot via
  `restore_downloads` ;
- **Banc inter-démons** : `interop_messaging_e2e.ps1` étendu —
  cycle groupe 3 démons + pièce jointe réelle, octets vérifiés.
