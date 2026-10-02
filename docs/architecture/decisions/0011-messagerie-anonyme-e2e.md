# ADR-0011 — Messagerie anonyme sur circuits e2e

Statut : **Acceptée** (2026-10-02) — sous réserve des bancs `MS-*`
de `docs/plans/bancs_tests.md` §4.11. Les questions ouvertes de la
version « Proposée » sont tranchées ci-dessous ; l'implémentation
suit la Phase 8 de `docs/plans/roadmap.md`.

## Contexte

La vision OnionBit est un démon unique combinant téléchargement
anonyme et messagerie de bout en bout sur la même infrastructure IPv8
(une identité, un overlay, un pool de circuits). La machinerie hidden
services est déjà complète et éprouvée en loopback :

- `join_swarm` / points d'introduction `IP_SEEDER` + annonce
  `DHTIntroPointPayload` dans la DHT IPv8 ;
- `create_rendezvous_point` (`establish-rendezvous` + cookie) et
  `create_e2e` (`link-e2e` → `linked-e2e` → `e2e_ready`) posant des
  `hs_session_keys` partagées sur le circuit ;
- cellules `data` sur le circuit lié (aujourd'hui : transport uTP
  BitTorrent, via adresses factices `circuit_id_to_ip`).

La messagerie peut réutiliser **intégralement** cette chaîne : un
« contact » est un swarm hidden dont le seul « seeder » est le
destinataire, et dont le contenu échangé est des trames texte au lieu
de blocs BitTorrent.

## Décisions proposées

### 1. Adressage

Un contact est identifié par la **clé publique LibNaCl de son
démon** (identité IPv8 déjà existante — aucune nouvelle identité).
Le swarm de contact dérive de la clé :

```text
messaging_hash(pk) = SHA1("onionbit messaging" || pk)
```

Le destinataire « seede » ce swarm (`join_swarm(messaging_hash(ma_pk),
hops, seeding=true)`) : points d'introduction `IP_SEEDER`, annonce
DHT périodique avec `seeder_pk = pk`. L'expéditeur résout les intro
points via `peers-request`/DHT, crée un `RP_DOWNLOADER` et lie un
circuit e2e — exactement le flux du téléchargement anonyme, sans
`.torrent`.

### 2. Transport applicatif

Après `e2e_ready` (circuit lié, clés de session posées), les trames
de messagerie circulent dans les cellules `data` du circuit lié :

- format minimal bencode/JSON `{type, id, ts, body}` borné en taille
  (ex. 32 Ko) — même discipline de parsing que les payloads tunnel
  (tests `dispatch_non_signe_ne_panique_pas`, fuzz `tunnel_payloads`) ;
- le récepteur distingue ces trames du flux uTP par le `info_hash` du
  circuit (swarm de messagerie) — pas de multiplexage sur le même
  circuit, un circuit e2e = une conversation ;
- accusé de livraison applicatif (`ack`) par trame — le tunnel
  garantit l'intégrité e2e (`hs_session_keys`), pas la remise.

### 3. Disponibilité — en ligne seulement (v1)

Pas de store-and-forward : les deux correspondants doivent être en
ligne (le swarm du destinataire n'existe que tant que son démon
tourne). Un envoi vers un contact hors ligne échoue proprement à
`peers-request`/DHT (aucun intro point) ou au timeout e2e. Le
store-and-forward (relais message persisté) est explicitement hors
périmètre v1 — il introduirait un fournisseur de persistance
observant les métadonnées.

### 4. Persistance locale

Tables dans `onionbit.db` (nouvelle migration) :
`contacts (pk, state[pending|active|blocked], retention_secs,
last_seq_in, recv_window)` et `messages (id, contact_pk, direction,
seq, ts, body, delivered)` — `seq`/`last_seq_in`/`recv_window`
portent l'anti-replay ; suppression réelle (`DELETE`). Historique
local non chiffré en v1 (cohérent avec le reste de la base ;
compromission d'endpoint hors périmètre du threat model — voir
« Persistance » dans les questions tranchées).

### 5. API/UI

- `GET /api/messages?contact=<pk_hex>` (historique),
  `POST /api/messages` `{contact, body}` (envoi),
  `GET/POST/DELETE /api/contacts` (dont acceptation/refus des
  `pending`) — section REST nouvelle, hors chemin Tribler
  (extension documentée dans `api_rest_mapping.md`) ;
- notifications SSE `message_received`/`message_delivered` via le
  `Notifier` existant ;
- UI Flutter : onglet Messagerie consommant l'API (hors périmètre de
  cette ADR).

### 6. Non-objectifs

- VoIP/audio : reporté (les circuits e2e pourraient porter des trames
  temps réel, mais les fluctuations de latence des tunnels rendent la
  qualité non garantie — à réévaluer après mesures).
- Groupes, présence, typing indicators : hors périmètre v1.
- Padding/obfuscation de trafic : hors périmètre (cf. threat model —
  pas de protection contre la corrélation temporelle globale).

## Conséquences

- Positif : réutilisation quasi-totale de la chaîne hidden services
  éprouvée ; un seul mécanisme e2e à sécuriser ; confidentialité du
  contenu par `hs_session_keys` (ChaCha20-Poly1305 e2e).
- Négatif : les points d'introduction du destinataire apprennent
  qu'on le contacte (métadonnée de fréquence, pas le contenu) ;
  l'adresse du destinataire reste cachée des relais intermédiaires
  mais le RP voit le cookie.
- Complexité : un service `messaging` dans `onionbit-core` +
  démultiplexage des `data` cells e2e par `info_hash` + table DB +
  endpoints REST.
- Interop : aucun client Tribler ne parle ce protocole — fonction
  OnionBit↔OnionBit uniquement (écart assumé, nouveau service).

## Questions tranchées (2026-10-02)

Le tunnel e2e fournit la **connectivité privée** ; il ne fait pas à
lui seul « une messagerie e2e sûre ». Chaque question ouverte de la
proposition a reçu une réponse actée ; les bancs `MS-*` de
`docs/plans/bancs_tests.md` §4.11 vérifient les décisions testables
avant toute diffusion.

### Format et cryptographie applicative

- **Format canonique versionné — acté** : trame bencode
  **déterministe** `{v, type, id, seq, ts, body, sig}` (clés triées,
  entiers minimaux), bornée à 32 Kio sur le fil. `v != 1` → rejet ;
  champ critique inconnu ou absent → rejet, jamais d'ignore
  silencieux. Le codec est l'unique point d'entrée du parseur et
  entre dans le corpus fuzz `tunnel_payloads`.
- **Authentification de l'émetteur — acté** : chaque trame est
  **signée Ed25519** par la clé IPv8 de l'expéditeur ; `sig` couvre
  la forme canonique des autres champs et est vérifiée contre la
  `pk` dont dérive le swarm (`messaging_hash(pk)`). Un pair qui
  connaît le swarm ne peut pas écrire « au nom de » son
  propriétaire — sans signature, la porte restait ouverte.
- **Séparation des clés — acté** : `hs_session_keys` = transport
  uniquement. La couche applicative dérive ses clés par HKDF-SHA256
  sur le secret e2e avec le domaine `"onionbit messaging v1"` ;
  rotation/perte d'un circuit casse la livraison, pas la session
  applicative, et inversement.
- **Ratchet — acté : sans ratchet en v1** (clés applicatives fixes
  par session e2e). Non-claim explicite : ni forward secrecy ni
  récupération post-compromission ; un ratchet minimal est en
  backlog (il change persistance et anti-replay).
- **Anti-replay / ordre / doublons — acté** : `seq` monotone `u64`
  par (contact, direction) + fenêtre de réception (bitmap des 64
  derniers seq) + dédup par `id` (16 octets aléatoires). Seq déjà vu
  ou hors fenêtre → drop sans réponse ; une réémission honnête
  (circuit reconstruit) tombe dans la dédup, jamais dans
  l'historique.

### Limites et abus

- **Taille maximale — acté** : trame ≤ 32 Kio sur le fil, `body` ≤
  30 Kio ; **pas de fragmentation** en v1 (un message long est
  refusé à l'émission ; entrant, le dépassement est un rejet, pas
  une troncature).
- **Anti-DoS applicatif — acté** : seau à jetons **par contact** et
  **global** (défauts ~2 trames/s/contact et ~10 trames/s global,
  configurables, `0` = illimité pour les bancs). Le démultiplexeur
  `data` vérifie taille et `v` **avant** tout parse — le coût par
  trame hostile est linéaire et borné. Les trames d'un circuit e2e
  non messagerie n'atteignent jamais le codec.
- **Modèle de signalement/abus — acté** : blocage local seulement —
  `contacts.blocked` refuse les trames du pair et son swarm cesse
  d'accepter ses liens e2e. Pas de fédération de signalement en v1.

### Métadonnées et cycle de vie

- **Consentement de contact — acté : explicite** : la première
  trame d'un inconnu est un `hello` placé en attente bornée
  (`pending`, capacité fixe, TTL 10 min) — un inconnu n'écrit ni
  l'historique ni un état non borné. Acceptation → contact actif ;
  refus → `blocked`. Les trames non-`hello` d'un inconnu : drop.
- **Présence — acté et assumé** : l'annonce DHT du swarm messagerie
  est périodique (`announce_interval` configurable, aligné sur les
  annonces de swarm existantes) — c'est un battement de cœur de
  présence observable par les intro points et la DHT, inscrit au
  threat model. Le mesh fingerprinting est rejoué messagerie active
  (banc MS-8) pour mesurer le delta.
- **Persistance — acté : en clair** : table `messages` et carnet de
  contacts en clair dans `onionbit.db`, cohérent avec le reste de la
  base (compromission d'endpoint hors périmètre du threat model).
  Pas de demi-mesure cérémonielle ; chiffrement disque
  (SQLCipher/clé OS) en backlog.
- **Expiration/suppression locale — acté** : suppression **réelle**
  (`DELETE`, pas de `deleted` logique) ; TTL optionnel par contact
  (`retention_secs`, `0` = conservation) — attention documentée à
  l'interaction WAL (`PRAGMA secure_delete=ON` si TTL utilisé).
- **Comportement offline — acté** : **échec immédiat visible** —
  envoi vers un contact hors ligne → `Undeliverable` à l'expiration
  du `peers-request`/timeout e2e borné ; pas de file d'attente ni de
  réémission (l'état API/UI est `failed`, sans boucle).

### Vocabulaire de garanties

Les claims de sécurité de la messagerie seront limités à ce que les
tests démontrent : confidentialité du **contenu** e2e, authentification
de l'émetteur, anti-replay testé. Non-claims explicites : résistance à
la corrélation de trafic, aux intro points malveillants observant la
fréquence de contact, à la compromission d'endpoint, et (en l'absence
de ratchet) à la compromission rétroactive des clés de session.

## Tests attendus à l'implémentation

Catalogue complet : `docs/plans/bancs_tests.md` §4.11 (`MS-*`) —
résumé :

- Loopback 2 nœuds : join swarm → annonce → lookup → RP → link-e2e →
  `hello`/consentement → message reçu et ACK renvoyé (réutilise
  `make_node`/`wait_two` des bancs `circuits_loopback`) — MS-1/MS-2.
- Protocole négatif : signature invalide, trame malformée, taille,
  `v` inconnu, seq rejoué/hors fenêtre, `id` dupliqué, trame
  non-`hello` d'inconnu — MS-3/MS-4/MS-5/MS-6.
- Contact hors ligne : échec borné `Undeliverable` sans boucle —
  MS-7.
- Non-fuite : aucun datagramme messagerie hors tunnel ; les swarms
  BitTorrent inchangés (les trames n'atteignent jamais la lane
  uTP) — MS-9.
- Fuzz : la trame entre dans le périmètre `tunnel_payloads`/
  `unsigned_dispatch` (parseur borné) — MS-4.
- Fingerprint : mesh avant/après avec annonces de présence actives —
  MS-8.
