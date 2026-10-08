# ADR-0017 — Transport furtif (anti-censure), mode OnionBit↔OnionBit

Statut : **Acceptée** (2026-10-08) — implantée sur le worktree
`adr17` (étapes 49-56, commits `2836f2c`..`ca05ca3`), banc
`bench_stealth_fingerprint.ps1` PASS (9/9 oracles, capture
socket réelle loopback), revue externe remplie
(`docs/security/revue_stealth.md`). Réserve honnête : le banc a
tourné en loopback via tap-proxy ; une campagne inter-machines
reste souhaitable pour le timing WAN. Suite du constat ADR-0015
§7 (fingerprinting) : `OBF` masque le contenu des trames ext mais
pas l'existence du protocole.

## Contexte

Menace considérée : un censeur étatique (Russie, Iran, Chine…) qui
**classifie et bloque** le trafic OnionBit/IPv8 — pas seulement un
observateur passif qui analyse le contenu.

Inventaire honnête de la surface identifiable aujourd'hui (avec ou
sans `ext.obf_enabled`) :

| Élément filaire | Visible pour un observateur |
| :--- | :--- |
| `community_id` (20 o) de chaque communauté — ext compris | constante publique, reconnait le protocole à chaque datagramme |
| Clé maîtresse ed25519 + signature sur chaque paquet `ez_send` | identité persistante du nœud |
| `hello` ext `{v, caps}` signé | clair par nécessité (négocie les caps) |
| `introduction-request/response` legacy | format IPv8 reconnaissable |
| Cellules tunnel (`create`/`data`…) | format IPv8 reconnaissable |
| Protocole BitTorrent (peer-wire, trackers, DHT mainline) | triviallement identifiable — indépendant d'IPv8 |
| Cadence/volume temporel | partiellement atténué par `hello_jitter` |

`OBF` (bit 0 de `hello.caps`) enveloppe les trames ext post-hello :
type de message, contenu et taille réels cachés. Il ne touche ni le
préfixe de communauté, ni le hello, ni la couche legacy — **un
censeur peut classifier « OnionBit » dès le premier datagramme** et
bloquer le protocole entier sans jamais lire un payload.

## Décision structurante

**La furtivité est binaire : impossible d'être à la fois non
classifiable et interopérable legacy sur le même nœud.**

Un mode hybride (transport obfusqué entre pairs ext + walk legacy
conservé pour Tribler) laisserait le discovery et les cellules tunnel
au format reconnaissable — le nœud resterait identifiable par la
couche legacy. Toute furtivité réelle exige donc un **mode dédié**
dans lequel :

1. tout le trafic sortant/entrant passe dans le transport morphed ;
2. le walk IPv8, la DHT publique et le peer-wire BitTorrent public
   sont désactivés ;
3. l'interop Tribler legacy est **entièrement sacrifiée** — les
   seuls interlocuteurs sont des nœuds OnionBit furtifs.

Ce serait le premier mécanisme OnionBit-only qui **ne coexiste pas**
avec le legacy — contrairement à tout ADR-0015 (ext est un ajout,
jamais une coupure).

## Design proposé

### 0. Modèle d'adversaire — « censeur » découpé (revue 2026-10)

| Adversaire | Objectif de défense réaliste |
| :--- | :--- |
| DPI passif | Masquer magic bytes, préfixes, clés, signatures, payloads |
| Probing actif | Répondre uniquement aux handshakes authentifiés (silence absolu) |
| Blocage par IP | Bridges non publics — réduction d'exposition, pas de garantie |
| Classification statistique | Réduire les signaux fixes, mesurer les distributions |
| Corrélation globale | **Hors périmètre** : volume et timing restent exploitables |
| Bridge compromis | Voit le client et le flux ; ne casse pas la crypto E2E |

### 1. Négociation hors-bande — le paradoxe du premier contact

La négociation de capacité par `hello.caps` est inutilisable ici : le
hello de négociation trahirait le protocole avant que le transport
obfusqué démarre. Le bootstrap furtif passe donc **hors-bande** :

- **Liens d'invitation** : `onionbit-bridge://<ip>:<port>#<bridge_pk>`
  — une entrée de « pont » (adresse + **clé de pont d'admission**,
  distincte de la clé maîtresse IPv8 du pair : la clé bridge sert
  l'admission et l'anti-probing, l'identité OnionBit n'est révélée
  qu'à l'intérieur du tunnel authentifié — frontière de corrélation
  identité/disponibilité). Ajoutée manuellement en config ou
  importée dans l'UI. Équivalent des bridges Tor (BridgeDB).
  *Compromis v1* : `bridge_pk` = pk maîtresse si le pont n'a pas
  configuré de clé dédiée — corrélation documentée comme limite.
- Le premier paquet vers un pont est déjà morphed et authentifié par
  la clé du pont — jamais de hello en clair.
- Découverte interne : les ponts introduisent des pairs furtifs par
  message ext `INTRO { addr, bridge_pk }` *à l'intérieur* du
  transport morphed — **anti-scraping en séquence graduée** : (1) le
  lien bridge suffit toujours à entrer ; (2) petit budget d'intros
  de démarrage sans condition de réputation (un nouveau client n'a
  aucun solde — le ledger ne doit jamais être une condition unique
  de bootstrap) ; (3) expansion de découverte conditionnée à la
  réputation/solde bilatéral du ledger (ADR-0015) ; (4) diffusion
  large réservée aux pairs ayant démontré une réciprocité. Table
  partitionnée (max 2-3 ponts par demande), TTL, pas de persistance
  par défaut. Sans cela, un seul lien compromis permettrait au
  censeur d'énumérer tous les ponts du réseau et de les blacklister
  par IP.

### 2. Format filaire — sans marqueur statique mesurable

Inspiration `obfs4`/`ntor` (conception éprouvée plutôt que
réinvention). **Formulation** : le transport est conçu pour
n'exposer aucun marqueur OnionBit/IPv8 statique dans le contenu, les
longueurs et le handshake ; sa résistance est *mesurée* contre les
classifieurs des bancs — sans garantie générale d'indiscernabilité
(« indiscernable de bruit » reste l'aspiration, pas une propriété
démontrable : tailles, directionnalité, rythme, volume, IP de ponts
connues, réaction aux pertes et corrélation temporelle restent des
signaux exploitables).

- **Aucune signature ni clé publique en clair** — authentification
  par preuve de connaissance de la clé partagée (X25519 dérivée de
  `(bridge_pk, sk_client)`), handshake type `ntor` en premier
  datagramme.
- **Éléments publics encodés Elligator2 — gate d'activation** : une
  clé X25519 brute n'est *pas* uniforme (bit de poids fort nul en
  représentation canonique ; ~50 % des chaînes de 32 octets échouent
  au test de résidu quadratique — les DPI avancés exécutent ces
  tests, cf. GFW). Les clés éphémères `X'`/`Y'` du handshake sont
  donc transportées sous forme de représentant Elligator2
  (RFC 9380 §6.7.1 couvre Curve25519). **Pas d'activation du mode
  sans encodage justifié** — jamais de fallback « X25519 brut ».
- **Anti-rejeu du handshake** : rejouer un premier datagramme
  capturé ne doit pas forcer de réponse (sinon le censeur confirme
  le pont par probing). Horodatage Unix dans le payload authentifié
  (fenêtre `±90 s`, configurable — la dérive d'horloge au-delà rend
  le handshake silencieusement infructueux : le diagnostic/UI doit
  suggérer la resynchronisation NTP) + filtre glissant des `X'` vus
  — structure à deux fenêtres temporelles (courante + précédente,
  purge à chaque bascule) sur `HashSet<[u8; 32]>` borné (~50 000
  entrées, rejet silencieux si saturé) : O(1), zéro allocation non
  bornée. Rejeu → silence absolu.
- **Silence absolu et uniforme** : trame non authentifiée → zéro
  réponse, zéro session, zéro allocation durable par IP source, zéro
  log bruyant (compteur atomique borné au plus). Le rejet doit être
  **identique quelle que soit la cause** — MAC invalide, horodatage
  hors fenêtre, `X'` rejoué, table saturée : tout comportement
  différenciable devient un oracle de probing.
  **Anti-amplification** :
  `octets sortants / octets entrants non authentifiés ≤ 1` — zéro
  sur requête invalide. **Budget CPU** : rate-limit pré-DH par IP
  source (contournable par spoofing/botnet — utile mais pas
  suffisant) + **plafond global d'opérations asymétriques/s**, la
  vraie dernière ligne de défense contre la saturation du thread
  réseau.
- **Tailles** : padding **additif** aléatoire (`inner_len +
  uniform(0, pad_max_extra)`), jamais de seaux réguliers, clampé à
  `STEALTH_MTU = 1280` — les cellules tunnel (~1400 o) + overhead
  stealth (préfixe communauté 22 o + compteur + tag AEAD 16 o)
  franchiraient la MTU et produiraient des fragments UDP, que les
  routeurs censurés droppent massivement. Le budget interne réduit
  (`STEALTH_MTU − overhead`, ~1200 o de payload) est propagé aux
  communautés — bridage automatique du chunking tunnel/HTTP en mode
  stealth. Limite assumée : l'additif conserve une corrélation taille
  inner→outer ; distribution ré-échantillonable laissée en option.
- **Cadence** : trafic de couverture optionnel (keepalive aléatoire)
  pour lisser les périodes de silence — coûteux en bande passante,
  opt-in (`stealth_cover_traffic`).
- **Rejeu des trames** : compteur dans l'AEAD, fenêtre de réception —
  propriétés déjà présentes dans `pairbox`/lanes e2e.

Domaine crypto distinct (`onionbit/stealth/v1`) de `ext-obf/v1` et
des `pairbox` tunnel — les blobs ne sont interchangeables sur aucun
domaine.

### 3. Mode `stealth` du daemon — rôles

Le mode furtif exige une **topologie à rôles** — un client censuré
seul ne télécharge rien (enclave vide). Trois rôles :

- **`client`** (censuré) : toutes interfaces claires coupées — seul
  le transport morphed vers les ponts ;
- **`bridge`** : accepte les handshakes furtifs entrants
  (distribution hors-bande du lien) — gate d'entrée, options
  d'allowlist de clients et de tickets d'invitation révocables ;
- **`gateway`** : transport furtif **plus** sortie BitTorrent
  publique (`EXIT_BT`) sur son propre socket — le peer-wire sortant
  n'est pas du trafic IPv8, il n'est pas morphé par construction.
  C'est ce rôle, hors zone de censure, qui rend le swarm mondial
  accessible aux clients. **Attention** : la gateway n'est pas
  furtive côté Internet public — elle protège le chemin
  client censuré ↔ gateway, pas son activité BitTorrent propre.

Nouvelle section de config `stealth` :

```jsonc
{
  "stealth": {
    "enabled": false,              // mode dédié — exclut ipv8.enabled legacy
    "role": "client",              // client | bridge | gateway
    "bridges": [],                 // liens d'invitation hors-bande
    "cover_traffic": false,        // keepalive aléatoire (coûteux)
    "pad_max_extra": 400,          // padding additif, clampé STEALTH_MTU
    "hs_timestamp_skew_secs": 90,  // fenêtre anti-rejeu handshake
    "replay_window": 256           // fenêtre trames par session
  }
}
```

Quand `stealth.enabled` :

- `TunnelCommunity` et la messagerie continuent (elles vivent
  *au-dessus* du transport — cellules relayées à l'intérieur du flux
  morphed) ;
- `DiscoveryCommunity`, DHT publique, session BitTorrent non-anonyme
  : **désactivés** chez `client` et `bridge` — le seul BitTorrent
  possible est celui des swarms cachés sur circuits (la sortie
  publique n'existe que chez `gateway`) ;
- la communauté ext tourne inchangée *dans* le transport (hello,
  attest, ledger, OBF conservent leur sémantique — OBF devient
  redondant à ce niveau et est désactivé) ;
- **kill switch furtif** : aucun datagramme clair ne sort du socket,
  jamais de repli silencieux vers le transport brut — même sur
  perte, timeout, NAT rebinding ou reprise de session ;
- toute configuration hybride (`stealth.enabled` × `ipv8` legacy)
  échoue **fermement** au démarrage, sur tous les chemins.

### 4. Validation — le banc qui fait foi

Un mode furtif ne se décrète pas, il se mesure : nouveau banc
`bench_stealth_fingerprint.ps1` dérivé de `fingerprint_mesh.ps1` —
oracle = **non-régression de signature** (pas preuve
d'indiscernabilité) : entropie par octet ~8 bits, aucune constante
de préfixe entre runs, aucune chaîne reconnue (`LibNaCL`,
`community_id`, `onionbit`), distribution des tailles lissée.
Cinq familles de tests (revue 2026-10) :

1. **Probing actif** : garbage, handshakes tronqués/falsifiés/
   rejoués → zéro réponse, zéro session, zéro croissance mémoire
   non bornée ;
2. **Amplification** : `octets sortants / octets entrants non
   authentifiés ≤ 1` ;
3. **Fuite legacy** : au niveau socket OS pendant transfert hidden
   + messagerie + ext — zéro préfixe IPv8, cellule brute, ez_send,
   peer-wire direct, DHT mainline, DNS/tracker inattendu ;
4. **Résilience** : perte 5-20 %, réordonnancement, duplication,
   NAT rebinding, redémarrage pont, expiration de session,
   saturation des files — jamais de repli vers le transport clair ;
5. **Classifieur comparatif** : corpus {DNS, QUIC, WireGuard, bruit,
   IPv8 normal, stealth}, features taille/direction/inter-arrivées/
   volume/ratio — mesure honnête de la séparation observable.

Le banc anti-probing est joué **avant** l'intégration daemon et la
revue de sécurité externe précède le passage à « Acceptée ».

## Alternatives rejetées

- **Obfusquer seulement ext** : déjà fait (OBF) — ne masque ni le
  protocole ni l'existence du nœud.
- **Domain fronting** : les grands CDN ont coupé la possibilité
  depuis ~2018 ; fragile et dépendant d'infrastructures tierces.
- **Morphing BitTorrent public** : impossible sans casser le
  peer-wire que les pairs legacy doivent parser — les swarms publics
  restent exclus du mode furtif (seuls les hidden swarms survivent).
- **Deux communautés ext** (legacy + stealth dans le même daemon) :
  le trafic legacy compromet la furtivité — un nœud ne peut pas être
  les deux à la fois.

## Limites assumées

- **Le volume de trafic reste un signal** : un nœud BitTorrent actif
  transfère des quantités caractéristiques de données — le morphing
  masque le *format*, pas le *volume*. Le cover traffic atténue mais
  ne supprime pas.
- **Bootstrap social** : la distribution des liens d'invitation est
  le point faible (comment trouver son premier pont ?) — problème
  connu de Tor, sans solution miracle.
- **Surface nouvelle** : un transport morphed est une surface
  crypto/protocolaire entière — fuzzing, bancs et relecture
  nécessaires avant tout défaut activé (discipline identique à OBF).
- **Corrélation bridge_pk ↔ identité** (v1) : si `bridge_pk` est la
  pk maîtresse faute de clé dédiée, distribuer le lien hors-bande
  lie durablement identité réseau et disponibilité du pont —
  compromis documenté, la clé dédiée est recommandée.
- **Énumération des ponts** : un lien compromis expose son pont à
  l'IP-blocking ; `INTRO` partitionné + gating réputation borne la
  fuite en cascade mais ne l'annule pas.
- **Étranglement UDP aveugle** : un censeur/FAI qui bloque ou
  étrangle tout UDP non-DNS/QUIC (Iran notamment) rend le transport
  inopérant — la furtivité de format ne protège pas contre
  l'interdiction de la couche. Évolution future possible : transport
  stream/TCP-TLS (mimicry), hors périmètre Phase 10.
- **Dépendance à l'horloge** : l'anti-rejeu du handshake suppose des
  horloges raisonnablement synchronisées ; dans les réseaux où NTP
  (UDP 123) est filtré ou spoofé, une dérive > fenêtre rend le
  premier contact muet — la fenêtre est configurable
  (`hs_timestamp_skew_secs`) et le diagnostic doit rendre cette
  cause visible.

## Conséquences

- Pas de code engagé tant que ce document reste « Proposée ».
- Si acceptée : Phase 10 de la roadmap — transport + config stealth +
  banc fingerprint ; `docs/security/fingerprinting.md` documente le
  nouveau modèle de menace.
- La compatibilité Tribler reste la règle pour le mode par défaut —
  `stealth` est un mode opt-in disjoint, jamais un comportement
  implicite.
