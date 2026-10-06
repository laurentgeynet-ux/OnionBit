# ADR-0017 — Transport furtif (anti-censure), mode OnionBit↔OnionBit

Statut : **Proposée** (2026-10-06) — exploration future, aucune
implémentation engagée. Suite directe du constat ADR-0015 §7
(fingerprinting) : `OBF` masque le contenu des trames ext mais pas
l'existence du protocole.

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

**La furtivité est binaire : impossible d'être à la fois indétectable
et interopérable legacy sur le même nœud.**

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

### 1. Négociation hors-bande — le paradoxe du premier contact

La négociation de capacité par `hello.caps` est inutilisable ici : le
hello de négociation trahirait le protocole avant que le transport
obfusqué démarre. Le bootstrap furtif passe donc **hors-bande** :

- **Liens d'invitation** : `onionbit-bridge://<ip>:<port>#<pk_hex>` —
  une entrée de « pont » (adresse + clé publique maîtresse du pair)
  ajoutée manuellement en config ou importée dans l'UI. Équivalent
  des bridges Tor (BridgeDB).
- Le premier paquet vers un pont est déjà morphed et authentifié par
  la clé du pont — jamais de hello en clair.
- Découverte interne : les ponts introduisent des pairs furtifs par
  attestation ext *à l'intérieur* du transport obfusqué (la
  communauté ext vit inchangée, sous la couche morphing).

### 2. Format filaire — indiscernable de bruit

Inspiration `obfs4`/`ntor` (conception éprouvée plutôt que
réinvention) :

- **Aucune signature ni clé publique en clair** — authentification
  par preuve de connaissance de la clé partagée (X25519 dérivée de
  `(pk_pont, sk_client)`), handshake type `ntor` en premier
  datagramme.
- **Tailles aléatoires** : padding aléatoire par datagramme
  (distribution uniforme/tronquée configurable), pas de seaux
  réguliers — les seaux constants sont eux-mêmes un signal.
- **Cadence** : trafic de couverture optionnel (keepalive aléatoire)
  pour lisser les périodes de silence — coûteux en bande passante,
  opt-in (`stealth_cover_traffic`).
- **Rejeu** : compteur de trame dans l'AEAD, fenêtre de réception —
  propriétés déjà présentes dans `pairbox`/lanes e2e.

Domaine crypto distinct (`onionbit/stealth/v1`) de `ext-obf/v1` et
des `pairbox` tunnel — les blobs ne sont interchangeables sur aucun
domaine.

### 3. Mode `stealth` du daemon

Nouvelle section de config `stealth` :

```jsonc
{
  "stealth": {
    "enabled": false,          // mode dédié — exclut ipv8.enabled legacy
    "bridges": [],             // liens d'invitation hors-bande
    "cover_traffic": false,    // keepalive aléatoire (coûteux)
    "pad_min": 64, "pad_max": 1200
  }
}
```

Quand `stealth.enabled` :

- `TunnelCommunity` et la messagerie continuent (elles vivent
  *au-dessus* du transport — cellules relayées à l'intérieur du flux
  morphed) ;
- `DiscoveryCommunity`, DHT publique, session BitTorrent non-anonyme
  : **désactivés** — le seul BitTorrent possible est celui des swarms
  cachés sur circuits ;
- la communauté ext tourne inchangée *dans* le transport (hello,
  attest, ledger, OBF conservent leur sémantique — OBF devient
  redondant à ce niveau et peut être désactivé).

### 4. Validation — le banc qui fait foi

Un mode furtif ne se décrète pas, il se mesure : nouveau banc
`bench_stealth_fingerprint.ps1` dérivé de `fingerprint_mesh.ps1` —
oracle = le trafic capturé doit être indiscernable de bruit uniforme
(entropie par octet ~8 bits, aucune constante de préfixe, aucune
clé reconnue, distribution des tailles lissée) face à un
classifieur naïf puis à `scapy`/analyse par motifs connus.

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

## Conséquences

- Pas de code engagé tant que ce document reste « Proposée ».
- Si acceptée : Phase 10 de la roadmap — transport + config stealth +
  banc fingerprint ; `docs/security/fingerprinting.md` documente le
  nouveau modèle de menace.
- La compatibilité Tribler reste la règle pour le mode par défaut —
  `stealth` est un mode opt-in disjoint, jamais un comportement
  implicite.
