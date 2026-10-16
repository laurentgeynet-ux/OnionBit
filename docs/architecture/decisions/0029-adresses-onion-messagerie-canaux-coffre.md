# ADR-0029 — Adresses `.onion` dans la Messagerie, les Canaux et le Coffre : fusion UI, transports séparés

Statut : **Proposée** (2026-10-17) — document de cadrage ouvert à
discussion ; aucune implantation n'est engagée tant que les questions
du §8 ne sont pas arbitrées.
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
pair héberge du chiffré sans le lire). **Condition de faisabilité
ouverte** : le support de l'autorisation client côté arti n'est
pas garanti à ce jour — le spike de l'étape 120 tranche.

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
(`onion-service` / `tor-hsservice`) — API **expérimentale** à ce
jour. Deux voies :

1. **Embedded** : `arti` avec le feature flag onion-service —
   autonomie totale, maturité à éprouver (étape 120 = spike).
2. **External** : `mode:"external"` + ControlPort d'un tor C —
   éprouvé, mais dépend d'un Tor installé (contraire à l'autonomie,
   repli documenté uniquement).

Le spike statue aussi sur : stabilité des intro points sur
Windows, coût batterie/données (mobile), autorisation client v3,
persistance des clés de service dans `state_dir/tor`.

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

## 8. Questions ouvertes (à arbitrer avant code)

1. **Protocole messagerie sur `.onion`** : même format de messages
   e2e porté sur un transport TCP/Tor dédié, ou endpoint REST/Tor
   minimal ? Réutilisation de `obmsg`/`obv1` ou nouveau type de
   trame ?
2. **Présence `.onion`** : ping périodique (coût/empreinte) ou
   tentative à l'envoi seulement ?
3. **Multi-plan d'une conversation** : fil unifié avec badge par
   message, ou conversations séparées par plan ?
4. **Coffre nomade** : priorité vs messagerie — le cas d'usage
   (accéder à son propre coffre) ne dépend d'aucun pair.
5. **Séquencement** : avant/après l'étape 117 (passerelle
   `EXIT_TOR`) et les 5 questions UX Tor en suspens ?
6. **Étendue du redesign chat** : refonte complète ou pastilles
   plan incrémentales ?

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
