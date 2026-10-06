# ADR-0015 — Extensions OnionBit : stratégie « legacy Tribler + ext OnionBit »

Statut : Acceptée (2026-10-05). Cadre de la Phase 9 de
`docs/plans/roadmap.md`. Phases 9a (comptabilité locale par pair),
9b (`OnionbitExtCommunity` + `hello` lazy), 9c (ledger bilatéral
signé), 9d (curation par attestations signées) et 9e (enveloppes
`OBF` négociées + jitter `hello`) livrées.

## Contexte

L'idée d'un registre de réputation pair-à-pair (type *TrustChain*)
pour récompenser les relais et filtrer le spam est pertinente — une
blockchain classique (état global, frais, latence de consensus) serait
en revanche un poison pour un daemon nomade. Mais l'analyse des sources
upstream (`<Tribler sources checkout>`, branche `main` post-8.4.3)
impose un constat : **TrustChain n'existe plus dans le réseau Tribler
actuel**.

| Événement upstream | Commit | Date |
| :--- | :--- | :--- |
| TrustChain retiré de py-ipv8 → déplacé vers AnyDex | `37b1923` (pyipv8) | 2020-12-07 |
| Suppression du `TrustGraph` dans Tribler | `7fe164612` | 2023-12-08 |
| Suppression de `BandwidthCommunity` (blocs bilatéraux, `bandwidth.db`) | `959169ed1` | 2024-01-05 |

Le pyipv8 3.2.1 livré avec Tribler 8.4.3 n'a plus
`ipv8.attestation.trustchain` (seuls `identity`, `tokentree`, `wallet`
subsistent) et `src/tribler/core` n'a plus `bandwidth_accounting`. Les
seuls vestiges sont un commentaire « bandwidth payouts » sur
`remove_tunnel_delay` et une notification
`tribler_torrent_peer_update` (`balance = dtotal`) que rien ne
consomme. **Réimplémenter le format historique n'apporterait donc
aucune interopérabilité** : un Tribler 8.x droppe silencieusement tout
préfixe de communauté inconnu.

Décision stratégique : le protocole partagé avec Tribler 8.x
(« legacy ») reste **strictement inchangé** ; les fonctions nouvelles
vivent dans une extension parlée **uniquement entre nœuds OnionBit**,
développée from scratch en Rust — sans reprendre le format TrustChain
de pyipv8 2.x, qui resterait mort-né faute d'interlocuteurs.

## Décisions

### 1. Coexistence filaire : préfixe dédié, jamais de champ partagé

Le `UdpEndpoint` dispatche déjà par préfixe (`community_id` 20 octets +
version). L'extension obtient son propre `community_id` constant —
impossible à collisionner avec les ID Tribler figés. Aucun bit ni champ
n'est ajouté aux formats partagés (`extra_bytes` des introductions,
flags tunnel, trames de cellules) : un tel bit serait un tag de
capacité visible par tout sniffer et tout pair legacy.

### 2. Découverte lazy/opportuniste — pas de walk dédié (Phase 9b — livrée)

Une marche aléatoire sur un préfixe inconnu annonce littéralement
« OnionBit tourne sur cette IP » à tout observateur passif. En v1,
l'extension **ne fait pas sa propre marche** : elle s'appuie sur les
pairs déjà découverts via les communautés legacy et envoie un `hello`
opportuniste ; un pair qui ne répond pas (Tribler 8.x) n'est plus
sollicité sur ce canal. Le peer set de l'extension *est* la population
OnionBit — la négociation de capacité est implicite.

Compromis honnête : le `hello` reste un signal observable. On minimise
l'exposition, on ne la supprime pas — même statut que la métadonnée de
présence `messaging_hash(pk)` (MS-8, `threat_model.md`).

Une **seule** communauté d'extension avec `msg_id` par fonction
(`hello`, `ledger_*`, `attest_*`, `obf_*`), trames `{v, ...}`
versionnées façon messagerie — plutôt que N communautés à peer sets et
surfaces de fuzzing distincts.

Implémentation (`onionbit-ipv8::ext`) : `EXT_COMMUNITY_ID` =
`922d2ad9ce00b0d84952638cfc227dc1343c1aa4` =
`sha1("OnionBit extension community")` (constante de domaine, aucune
clé maîtresse) ; `hello` = `{v: u8, caps: u64}` signé `ez_send`,
bitmap `caps` extensible — bit 0 `CAP_OBF_V1` (enveloppes OBF,
si `ext/obf_enabled`), bit 1 `CAP_MSG_V1` (messagerie anonyme
ADR-0011, si `tunnel_community/messaging_enabled` et tunnel actif —
pure annonce, les liaisons e2e restent dans le tunnel) —,
cooldown par pair pour la re-sollicitation comme pour la réponse
(pas de ping-pong) ; `ext/enabled` défaut **on** depuis la
validation terrain (2026-10-06 — T1 silence legacy + soak 10/10 +
bancs interop verts) : `off` par défaut rendait l'interconnexion
OnionBit↔OnionBit impossible en pratique (`bench_ext_interconnect`).
Le HELLO signé en clair expose exactement ce que le discovery IPv8
legacy publie déjà (clé maîtresse + adresses) ; `enabled=false`
reste honorable pour redevenir muet.

### 3. Comptabilité locale d'abord (Phase 9a — livrée)

Avant tout protocole signé : une **mesure pure**, zéro octet sur le
fil. Ce que le filaire permet d'attribuer honnêtement :

- `bytes_served[pk]` — octets transportés pour un circuit joint par
  `create` direct : le `requester` du `create` **est** l'initiateur
  (seul le premier saut connaît son identité ; un saut intermédiaire
  ne voit que ses voisins). Volume = `exit_sockets[cid].bytes_total`
  ou, pour une paire de relais, la somme des deux routes divisée par
  2 (`relay_cell` incrémente `bytes_up` **et** `bytes_down` du même
  datagramme sur la route entrante — chaque route porte 2× son
  volume réel).
- `bytes_used[pk]` — octets transportés par chaque **saut vérifié de
  nos propres circuits** : l'initiateur connaît toute la route, et
  chaque saut a réellement porté le volume (`bytes_up + bytes_down`).

Asymétrie assumée : quand *nous* servons en position intermédiaire,
l'initiateur nous crédite de son côté (`used`) sans que nous puissions
l'attribuer (`served` non porté). La balance bilatérale reste une
approximation honnête de la contribution réciproque.

Comptage par deltas à chaque tick de maintenance (live, crash-safe)
+ flush final au retrait — jamais de double comptage. Persistance par
trait `PeerStatsStore` injecté (pattern `GuardStore`/`DbGuardStore` :
`InMemory` par défaut, adaptateur `DbPeerStatsStore` dans `core`,
table `peer_stats`, migration v16).

### 4. Politique d'admission : déficit borné, sous pression seulement

- `ledger_enforce = false` **par défaut** (mesure d'abord, promotion
  après validation terrain — même discipline que les guards).
- Quand l'application est active **et** que `joined >= soft_cap` :
  un `create` n'est admis que si `served - used <= max_deficit_bytes`.
- Le crédit de démarrage *est* `max_deficit` : un pair inconnu a
  `served = used = 0` → admis ; sa dette cumule jusqu'au plafond.
- Sous le `soft_cap`, tout est admis — comportement pyipv8 exact,
  aucun gel du bootstrap.
- Pas de deadlock de remboursement : un pair rembourse en servant
  **nos** circuits (`used` croît) — c'est nous qui choisissons nos
  sauts, l'équité émerge sans coordination.

### 5. Ledger bilatéral signé (Phase 9c — livrée)

La seule chose que la comptabilité locale ne peut pas faire :
**prouver sa contribution à un pair qui ne nous a jamais rencontré**
(priorité chez un exit étranger). Si ce besoin se démontre :

- format natif `{pk_a, seq_a, prev_a, pk_b, seq_b, prev_b, tx, sig_a,
  sig_b}` — inséré dans les deux chaînes, validation O(1) ;
- *sign-then-serve par tranches* : le relai n'accorde la tranche
  suivante que si le bénéficiaire a co-signé la précédente — le refus
  de signature devient auto-sanctionné ;
- fork : détection par gossip des *heads* (`pk → seq max`), preuve
  conservée et propagée — jamais de « consensus » ;
- `tx` **chiffré pour la paire** (secret X25519 des deux clés) :
  seuls `pk`/`seq`/`prev_hash` restent en clair pour la détection de
  forks — le *bandwidth crawler* passif de Tribler 7.x (reconstitution
  du graphe social et des heures d'activité) est un anti-patron.

Implémentation (`onionbit-ipv8::ext::ledger` + `onionbit-db::ext_ledger`
+ `onionbit-core::ext_ledger_store`) : `LEDGER_PROPOSE`/`SEAL`/`HEAD`/
`REJECT`/`FORK` bornés (`LEDGER_FRAME_MAX`, budget par émetteur).
Identité de proposition `proposal_id = sha256(champs + sig_a)` —
indépendante de `sig_b` : le sceau *remplace* la proposition en vol,
un `proposal_id` différent à la même position `(pk, seq)` est une
équivoque propagée (`FORK`, preuve conservée). `settle_tick` relance
les propositions en vol (borné par `ledger_max_retries`), propose la
tranche suivante aux pairs dont `served − settled ≥ ledger_tranche_bytes`,
et ne gossip que des têtes **scellées**. Le `REJECT` du bénéficiaire
porte sa vraie tête + sa mesure — la proposition corrigée plafonne à
`measured + dérive` (`ledger_drift_*`). Le veto tunnel `admit`
(`ledger_enforce`, défaut `false` = mesure d'abord) refuse le service
au-delà d'une tranche non signée. Bancs T5a/T5b/T5c verts
(`docs/plans/bench_adr0015/`).

### 6. Curation — indépendante du ledger (Phase 9d — livrée)

Attestations signées Ed25519 sur `channel_node` (déjà supporté par le
schéma) + listes de curateurs suivis ; score de confiance calculé
**localement**. Pas de dépendance à la réputation bande passante —
une chaîne IPTV est une clé publique, pas un portefeuille.

Implémentation (`onionbit-ipv8::ext::attest`) : l'`Attestation` est
**auto-portante** — `curator` et `signature` voyagent dans le payload
(domaine de signature séparé `onionbit/attest/v1`), donc ré-émissible
par un tiers et reverifiable depuis le stockage. Propagation `ATTEST`
par gossip borné : stockage et re-émission réservés aux curateurs
suivis (`ext/curators` + soi) — borne Sybil : un flot de signatures
valides de curateurs inconnus est droppé. Conséquence
assumée en v1 : une attestation ne chemine que par les nœuds qui
suivent son curateur — la qualité de propagation croît avec
l'adoption des curateurs, jamais l'inverse ; la découverte de
curateurs reste hors bande (config). Dedup `(curateur, kind, sujet)`
*latest-wins* (`ts` strictement plus récent, `ts` futur borné par
`attest_max_future_skew`) — un vieux verdict rejoué ne ré-écrit pas.
Persistance : table `attestations` (v17), trait `AttestationStore`
injecté, `DbAttestationStore` (core). Score `+1`/`-1` par curateur
suivi exposé par `GET /api/ipv8/ext/trust/{kind}/{subject}` ;
publication `POST /api/ipv8/ext/attest`.

Kinds de sujet (`u8`, longueur de `subject` fixe par kind) :
`1=infohash` (info-hash 20 o), `2=channel` (`LibNaClPK` 74 o),
`3=identity` (`pk_bin` du pair — 74 o, mêmes octets que `curator` et
que les clés des contacts messagerie). `identity` porte la confiance
« utilisateur » (verdict d'un pair sur la clé d'un autre) **et** la
liste d'amis auto-signée : une auto-attestation `endorse` sur la clé
d'un contact est une entrée portable — les suiveurs la retiennent et
la re-gossipent, un nouveau device partageant la même identité
(ADR-0016) la retrouve dans son store ext (best-effort : la
récupération dépend des suiveurs présents ; les pseudonymes restent
locaux, le format signé n'a pas de champ libre). Correction : la
longueur `channel` était 42 — une `LibNaClPK` complète fait 74 o
(`LibNaCLPK:` + crypt_pk + vk) ; aucune attestation canal réelle
n'était possible avec la borne historique.

**Chemin de réception ordonné du moins coûteux au plus coûteux**
(durcissement post-revue) : budget `ATTEST` par émetteur
(`ext/attest_rate_*` — borne CPU face aux rafales, table bornée
contre les clés Sybil fraîches) → borne de taille
(`ATTEST_FRAME_MAX`) → parse borné → **préfiltre curateur suivi**
(un curateur inconnu est droppé avant toute crypto *applicative* —
la vérification Ed25519 de l'attestation n'est payée que pour du
potentiellement nouveau d'un curateur suivi ; la signature
**transport** `ez_send` du paquet, nécessaire à l'identification de
l'émetteur pour le budget par clé, est vérifiée au parse comme pour
tout message signé — le gain porte sur la crypto de l'objet
métier, pas sur celle du transport) → lookup dedup (Ed25519
déterministe : mêmes champs = octets déjà vérifiés — rejeu/stale
absorbé sans `verify`) → borne `ts` futur → `verify` → **conflit
d'équivoque** :
même `(curateur, kind, sujet)` + même `ts` + verdict différent est
rejeté — ni écrasement ni ré-émission (deux signatures valides pour
le même `ts` = équivoque avérée, logguée) → stockage → ré-émission.
Compteurs `attest_rx/dropped/stored/tx` exposés par
`GET /api/ipv8/ext` — oracles des bancs (drops visibles, extinction
du gossip : `tx → 0`).

**Suppression d'un curateur suivi** : ses attestations *restent* en
base (rien n'est purgé — elles restent vérifiables et visibles dans
`attestation_count`) mais cessent de compter dans `score` — le filtre
`is_followed` s'applique au calcul, pas au stockage ; ré-ajouter le
curateur réintègre ses verdicts. `ext/curators` est pris en compte à
la création de la stack (redémarrage).

### 7. Anti-DPI — dernier, négocié, mesuré *(Phase 9e livrée)*

La baseline de discrétion est la **parité d'empreinte avec Tribler**
(`docs/security/fingerprinting.md` déjà instrumentée), pas un profil
« pseudo-WebRTC » qui créerait une troisième empreinte. Toute
obfuscation est une capacité annoncée par `hello`, opt-in, jamais
activée avec un pair legacy.

**Mesure préalable (banc 47.1)** : en mesh contrôlé (`-WithExt
-WithAnonDownload`, 15 min), le volume ext total est **~1,5 Ko
(~0,03 % de l'endpoint)** — 4 `hello`, un pic `ATTEST` auto-extinct,
0 `LEDGER_*` (le ledger n'émet que sur tranches de trafic réel).
L'empreinte exploitable n'est donc pas le volume mais le *contenu* :
`msg_id` et tailles lisibles dans l'enveloppe `ez_send`, cadence
exacte du `hello`.

**Mécanisme livré** — deux briques :

- **`OBF` (`msg_id` 8), enveloppe opaque de paire.** `{v, blob}` où
  `blob = pair_seal_in(inner)` — ChaCha20-Poly1305 sous la clé X25519
  de la paire, domaine HKDF dédié `onionbit/ext-obf/v1` (distinct du
  `pairbox` des `tx` : les blobs ne sont pas interchangeables).
  `inner = msg_id ‖ len ‖ payload ‖ pad` : le type et la taille
  réelle ne quittent pas l'AEAD. Padding au multiple supérieur de
  `ext/obf_pad_bucket` (256 par défaut) — `hello`-sized et
  `attest`-sized convergent vers la même classe. Émission : `OBF`
  uniquement si `ext/obf_enabled` **et** le pair a annoncé
  `CAP_OBF_V1` (bit 0 de `hello.caps`) — sinon clair, la
  compatibilité intra-ext est préservée. Réception : budget partagé
  `LEDGER_*` avant déchiffrement (borne AEAD), `obf_dropped` sur
  version/AEAD/inner malformés, `OBF` imbriqué refusé (pas de
  récursion), dispatch de l'inner dans le chemin `on_packet` normal
  — la signature `ez_send` de l'enveloppe authentifie l'émetteur, le
  contenu interne garde ses propres signatures (attestation auto-
  portante, lien bilateral).
- **Jitter `hello`** (`ext/hello_jitter_pct`, 25 % par défaut) :
  le sondage n'est plus à cadence exacte `hello_interval` — intervalle
  + tirage uniforme `0..=25 %` — la périodicité était le signal
  temporel le plus marquant de la mesure.

Non-couvert assumé : le préfixe `community_id` reste visible
(l'existence du trafic ext est une métadonnée déclarée), et le `hello`
initial est en clair — c'est le bootstrap de la négociation.

### 8. `network-policy` sans exception

Les extensions subissent exactement les mêmes règles (filtrage
`exit_data`, anti-SSRF, kill switch) — aucun affaiblissement n'est
introduit pour l'extension.

## Conséquences

- **Positif** : aucune rupture de l'interop Tribler 8.x validée (étape
  12) — l'extension ne peut pas casser le réseau partagé ; format
  libre corrigeant d'emblée les leçons de l'échec TrustChain (refus
  de signature, crawler passif, forks) ; la comptabilité locale seule
  apporte déjà l'essentiel du bénéfice incitatif.
- **Négatif** : les fonctions ext sont OnionBit-only ; la population
  OnionBit reste infime face au mesh Tribler — la réciprocité n'a de
  sens statistique que si les relais OnionBit deviennent nombreux.
- **Fingerprint** : toute activité ext est observable ; documentée
  comme métadonnée assumée, jamais présentée comme de la discrétion.

## Tests attendus (Phase 9a)

- comptage servi/utilisé exact sur circuit 2 sauts loopback ;
- tick + retrait sans double comptage ;
- persistance redémarrage (`DbPeerStatsStore`, aller-retour DB) ;
- admission : crédit de démarrage (pair inconnu admis), refus au
  déficit sous pression, `enforce = false` → comportement pyipv8
  exact.

## Tests attendus (Phase 9b — livrés)

- `hello` bilatéral loopback : marquage mutuel des deux pairs,
  `caps` propagé, une seule réponse par cooldown (pas de ping-pong) ;
- sondage limité aux pairs vérifiés, borné par `fanout` ; pair muet
  (Tribler) non re-sollicité pendant `hello_cooldown` ;
- version inconnue → drop sans marquage ext ; `msg_id` inconnu →
  ignoré ; datagrammes tronqués/signature fausse → drop sans panic ;
- fuzz : cible `ext_packet` + surface dans la régression stable ;
- `community_id` sans collision avec les ID Tribler figés.

## Tests attendus (Phase 9d — livrés)

- `Attestation` : round-trip sign/verify, domaine de signature
  séparé (rejeu croisé impossible), formes rejetées (kind/verdict/
  longueur sujet, troncature) ;
- gossip loopback A→B→C : propagation aux suiveurs, ré-émission par
  un non-curateur, drop chez un pair qui ne suit pas le curateur ;
- signature corrompue / `ts` futur au-delà de la dérive → jamais
  stockée ; rejeu identique → absorbé sans ré-émission ; verdict
  plus récent → remplace et inverse le score ;
- store mémoire + DB : dedup latest-wins, `latest` ordonné, borne
  d'éviction ; adaptateur `DbAttestationStore` aller-retour ;
- API : 404 ext désactivée, 400 corps invalide avant stack ;
- fuzz : `Attestation::unpack` dans `ext_packet` + régression
  stable ;
- durcissement : conflit même `ts` (`endorse` accepté puis `flag`
  rejeté — store/score inchangés, aucune ré-émission), préfiltre
  curateur non suivi observable par compteurs, budget `ATTEST`
  par émetteur (au-delà du cap : drops comptés, stores bornés).
