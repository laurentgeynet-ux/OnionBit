# ADR-0011 — Messagerie anonyme sur circuits e2e

Statut : Proposée (2026-10-02) — design seulement, aucune
implémentation. À valider avant tout code.

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

Table `messages` dans `onionbit.db` (nouvelle migration) :
`(id, contact_pk, direction, body, ts, delivered)` — historique local
non chiffré (cohérent avec le reste de la base ; compromission
d'endpoint hors périmètre du threat model).

### 5. API/UI

- `GET /api/messages?contact=<pk_hex>` (historique),
  `POST /api/messages` `{contact, body}` (envoi) — section REST
  nouvelle, hors chemin Tribler (extension documentée dans
  `api_rest_mapping.md`) ;
- notification SSE `message_received` via le `Notifier` existant ;
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

## Tests attendus à l'implémentation

- Loopback 2 nœuds : join swarm → annonce → lookup → RP → link-e2e →
  message reçu et ACK renvoyé (réutilise `make_node`/`wait_two` des
  bancs `circuits_loopback`).
- Contact hors ligne : échec borné sans boucle infinie.
- Non-régression : les swarms BitTorrent restent inchangés (les
  trames messagerie n'atteignent jamais la lane uTP).
- Fuzz : la trame de messagerie entre dans le périmètre
  `tunnel_payloads`/`unsigned_dispatch` (parseur borné).
