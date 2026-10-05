# ADR-0015 — Extensions OnionBit : stratégie « legacy Tribler + ext OnionBit »

Statut : Acceptée (2026-10-05). Cadre de la Phase 9 de
`docs/plans/roadmap.md`. La Phase 9a (comptabilité locale par pair)
est livrée avec cette ADR ; les phases 9b+ appliquent les décisions
posées ici sans être encore implémentées.

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

### 2. Découverte lazy/opportuniste — pas de walk dédié

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

### 5. Ledger bilatéral signé — différé (Phase 9c, conditionnelle)

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

### 6. Curation — indépendante du ledger

Attestations signées Ed25519 sur `channel_node` (déjà supporté par le
schéma) + listes de curateurs suivis ; score de confiance calculé
**localement**. Pas de dépendance à la réputation bande passante —
une chaîne IPTV est une clé publique, pas un portefeuille.

### 7. Anti-DPI — dernier, négocié, mesuré

La baseline de discrétion est la **parité d'empreinte avec Tribler**
(`docs/security/fingerprinting.md` déjà instrumentée), pas un profil
« pseudo-WebRTC » qui créerait une troisième empreinte. Toute
obfuscation est une capacité annoncée par `hello`, opt-in, jamais
activée avec un pair legacy.

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
