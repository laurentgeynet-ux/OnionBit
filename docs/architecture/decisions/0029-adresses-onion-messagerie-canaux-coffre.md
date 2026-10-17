# ADR-0029 — Adresses `.onion` dans la Messagerie, les Canaux et le Coffre : fusion UI, transports séparés

Statut : **Acceptée** (2026-10-17) — questions du §8 arbitrées à
l'étape 121 ; les jalons 122+ sont précisés en conséquence.
Plan d'implantation : `docs/plans/roadmap_adr0029.md` (étapes 120+ —
jalons de décision avant jalons de code).
Succède à la « trajectoire phase 3 » de l'ADR-0028, qu'elle engage.

## Contexte

L'ADR-0028 a livré Tor **sortant** (recherche de métadonnées sur
`.onion` / exit TLS) et documenté une trajectoire « côté service » :
faire apparaître des adresses `.onion` dans les sous-systèmes
existants — Messagerie (ADR-0019/0026), Canaux (ADR-0025), Coffre
(ADR-0018/0027). La présente ADR transforme cette trajectoire en
projet, avec une exigence produit explicite :

> **Fusion dans l'interface uniquement.** Tor n'envoie pas dans
> OnionBit, OnionBit n'envoie pas dans Tor — les deux plans restent
> complètement séparés, comme aujourd'hui. L'UI présente les deux
> familles d'adresses côte à côte ; chaque flux emprunte le plan de
> son adresse.

La séparation n'est pas qu'un choix d'hygiène : Tor ne transporte
que du TCP — l'overlay OnionBit (uTP/UDP) ne peut techniquement pas
y passer. Aucun pont, aucune translation, aucun fallback
inter-plans : un message adressé à un `.onion` part sur un circuit
Tor ; un message adressé à une clé OnionBit part sur l'overlay IPv8.

**Hypothèse runtime acquise (ADR-0028, décision 2026-10)** :
`tor.enabled` désigne une **lane permanente** — bootstrap au
démarrage du daemon et à l'activation à chaud, client vivant en
continu. C'est exactement le prérequis de cette ADR : un service
`.onion` joignable (messagerie, coffre nomade) suppose un client
arti toujours chaud, pas monté à la demande par une recherche. Le
hot-swap `swap_tor_stack` garantit en outre que les futurs
consommateurs Tor obtiennent le runtime courant, pas un clone
figé au boot.

## Décision proposée

### 1. Deux plans, une interface

| | Plan OnionBit | Plan Tor |
|---|---|---|
| Adresse | clé publique `ipv8` (contact/canal) | `xxxx.onion` (v3, 56 cars) |
| Transport | circuits uTP multi-sauts | circuits arti (TCP) |
| Joignabilité | async (boîtes aux lettres ADR-0026) | **en ligne requis** (service hébergé) |
| Badging UI | badge « OnionBit » (vert) | badge « Tor » (violet) — couleurs distinctes |

L'utilisateur voit une seule liste de conversations, de canaux et
de coffres ; chaque élément porte une pastille de plan indiquant
par quel réseau il est atteint. **Jamais** une adresse d'un plan
n'est utilisée pour joindre l'autre plan.

### 2. Identité : adresse `.onion` dérivée, jamais la clé `ipv8-sign`

Reprise inchangée du correctif de l'ADR-0028 : réutiliser la clé
d'identité IPv8 (ed25519) comme clé de service caché rendrait le
`.onion` strictement équivalent à la clé publique — pont de
corrélation inter-plans gratuit. La graine identité (BIP39) dérive
une clé de service séparée sous le domaine HKDF
**`onionbit/identity/tor-hs/v1`** : même phrase, deux plans non
liables. Pour les personas/canaux *publics*, la liaison est opt-in
et explicite : le canal signe son adresse `.onion` → adresse
auto-certifiante (jamais implicite).

### 3. Messagerie (ADR-0019/0026)

Modèle Briar : un contact Tor est joignable via son service caché,
e2e sans serveur, traversée NAT/CGNAT universelle, anonymat
symétrique (3+3 sauts). Contraintes connues :

- le `.onion` n'est joignable que **pendant que le pair héberge**
  (les circuits d'introduction meurent avec lui — aucune
  persistance façon DHT) → l'asynchronisme reste aux boîtes aux
  lettres store-and-forward, elles-mêmes exposables en `.onion`
  chez des pairs volontaires ;
- latence d'établissement en secondes → texte OK, temps réel exclu ;
- les deux plans cohabitent **dans le même fil de conversation** si
  le contact possède les deux adresses : l'UI choisit le plan
  joignable (badge du plan réellement emprunté sur chaque message).

### 4. Canaux (ADR-0025)

Le catalogue `mdblob` signé d'un canal peut être servi en HTTP sur
le `.onion` du créateur — complément au `RemoteSelect` UDP et chemin
de survie quand l'overlay est filtré. Le contenu reste vérifié par
la même signature (les entrées sont signées par la clé du canal,
pas par le transport). Disponibilité : le créateur en ligne, sauf
miroirs d'abonnés volontaires (opt-in).

### 5. Coffre-fort (ADR-0018/0027)

Cas le plus propre : accès nomade à son **propre** explorateur
privé via `.onion` + *client authorization* v3 (x25519) — la clé
client reste chez le propriétaire, le service ne sert que le
manifeste chiffré ; réplication aveugle coffre→coffre possible (le
pair héberge du chiffré sans le lire). **Autorisation client** :
`tor-hsservice` 0.47 expose `restricted_discovery` (clés
`descriptor:x25519:<base32>` par nickname, fichiers `.auth` ou map
statique) — mais derrière la feature `restricted-discovery`
marquée `__is_experimental` chez arti-client : à activer en
connaissance de cause (API non garantie semver).

### 6. Interface (ADR-0021)

- Pastille de plan distincte partout où une adresse apparaît :
  contacts, canaux, coffres — **couleurs différentes** (violet Tor
  / vert OnionBit), icône dédiée, tooltip explicite (« joignable
  seulement quand le pair est en ligne » pour `.onion`).
- Pastille de présence par plan : overlay = circuits ; `.onion` =
  « en ligne / hors ligne » (pas de présence continue).
- **Chat UI modernisée** : la page Messagerie gagne un rafraîchissement
  visuel (bulles, regroupement, états d'envoi par plan) — chantier
  UI propre, découplé du backend.
- Aucune adresse `.onion` affichée en v1 du chantier tant que le
  service caché n'est pas hébergé (règle ADR-0028 §8 — ne jamais
  afficher une joignabilité inexistante).

## Prérequis technique

Héberger un service caché exige le côté **serveur** d'arti
(`onion-service` / `tor-hsservice`). Deux voies :

1. **Embedded** : `arti` avec le feature flag onion-service —
   autonomie totale. **Verdict du spike 120 (2026-10-17, arti
   0.47 sous Windows) : GO** — voir ci-dessous.
2. **External** : `mode:"external"` + ControlPort d'un tor C —
   éprouvé, mais dépend d'un Tor installé (contraire à l'autonomie,
   repli documenté uniquement).

**Résultat du spike** (`onionbit-tor --features hs`,
`examples/hs_probe.rs`, réseau Tor réel) :

- Service v3 lancé via `TorClient::launch_onion_service`
  (`OnionServiceConfig` + nickname) : bootstrap ~40 s puis
  `Bootstrapping → Running`, descripteur publié, adresse
  `wzftlyph…svyd.onion` servie — **fonctionnel sous Windows**.
- **Persistance des clés** : relance sur le même `state_dir` →
  **même adresse** (keystore `state_dir/state/keystore` +
  `hss/`) — condition d'une identité `.onion` stable validée.
- **Autorisation client x25519** (requis coffre, §5) : existe via
  `RestrictedDiscoveryConfig` mais feature
  `restricted-discovery` = `__is_experimental` — activable, API
  non semver.
- Intro points : 3 par défaut (configurable ≤ 20), publication et
  tenue gérées par `tor-hsservice` — aucun code de maintien à
  écrire ; le coût est celui des circuits de garde (mutualisés
  avec la lane client déjà permanente).
- Empreinte build : +5 crates (`tor-hsservice`, `tor-persist`,
  `serde_bytes`, `k12`, `growable-bloom-filter`, `xxhash-rust`)
  derrière la feature `hs` — opt-in, sans coût pour la v1.

## Sécurité / vie privée

- **Corrélation inter-plans** : un adversaire observant le même
  contenu sur les deux plans (canal servi en UDP **et** en
  `.onion`) peut relier par timing/empreinte — la liaison opt-in
  est la seule exception assumée ; documenter au THREAT-MODEL.
- **Joignabilité = présence** : un `.onion` répond ⇒ le pair est
  en ligne — méta-information d'activité, assumée (Briar idem).
- **Pas de secret croisé** : la clé `.onion` dérivée ne lit ni le
  manifeste privé ni les boîtes OnionBit sans autorisation client ;
  `state_dir/tor` ne contient aucun secret d'identité.
- **Politique réseau** : les adresses `.onion` des contacts/canaux
  s'enregistrent via `TorTargetPolicy` — l'allowlist s'étend aux
  pairs approuvés, jamais de fetch libre.

## 8. Questions ouvertes — arbitrées (étape 121, 2026-10-17)

1. **Protocole messagerie sur `.onion`** → **trame TCP fine portant
   `OBV1`/`obmsg` inchangé**. La charge utile e2e (sérialisation,
   ChaCha20-Poly1305, validation, stockage) est identique sur les
   deux plans ; seul le transport change (`DataStream` arti vs
   datagramme overlay). Rejeté : endpoint REST/Tor minimal — surface
   HTTP superflue et second format à auditer.
2. **Présence `.onion`** → **tentative à l'envoi uniquement**
   (modèle Briar). Pas de ping périodique : circuits tenus =
   empreinte d'analyse de trafic + coût gardes/CPU. Rendez-vous à
   l'envoi ; échec → repli boîte aux lettres (ADR-0026). Cache
   « dernier contact vu » + bouton manuel « tester la joignabilité »
   autorisé, jamais automatique.
3. **Multi-plan d'une conversation** → **fil unifié, badge plan par
   message**. Un contact agrège ses adresses ; chaque bulle porte
   la pastille du plan réellement emprunté (vert OnionBit / violet
   Tor). La liaison des deux adresses d'un même contact reste
   **explicite** — jamais inférée.
4. **Priorité** → **messagerie d'abord** (étapes 122-125), coffre
   en 127. La messagerie n'exige que l'API stable
   `launch_onion_service` et construit la machinerie mutualisable
   (dérivation HKDF, persistance, mapping service) ; le coffre
   repose sur `restricted-discovery` = `__is_experimental` — risque
   semver remis à plus tard.
5. **Séquencement vs étape 117 (`EXIT_TOR`)** → **indépendant** ;
   117 reste différée par design. La passerelle sert des pairs sans
   Tor (sortie exit, threat-model honeypot) — aucune dépendance avec
   l'hébergement de services.
6. **Étendue du redesign chat** → **incrémental borné**, étape 125 :
   bulles/regroupement/états d'envoi, pastilles plan, **refonte de
   la zone Contacts** (liste actuelle ambiguë — entrées et icônes
   confuses). Pas de refonte complète de l'architecture Flutter.

## Alternatives considérées

- **Pont Tor↔OnionBit** (relayer les messages d'un plan à l'autre) :
  rejeté par l'exigence produit — la fusion est au niveau UI,
  jamais au niveau transport (et Tor ne fait pas d'UDP).
- **`.onion` hébergé pour l'API complète** : rejeté — la surface
  admin REST n'a pas vocation à sortir du loopback ; seuls des
  services dédiés (messagerie, canal, coffre) sont exposés, chacun
  avec sa clé et sa politique.
- **Clé `ipv8-sign` comme clé de service** : rejeté (ADR-0028,
  §Trajectoire) — corrélation inter-plans gratuite.
