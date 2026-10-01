# ADR-0010 — Guard nodes : persistance du premier saut

Statut : Proposée (2026-10-01) — document de décision, **aucune
implémentation** avant acceptation.

## Contexte

Contre un attaquant qui contrôle une fraction `f` des relais, chaque
circuit construit est un tirage : la probabilité que *son* noeud
d'entrée ET *son* noeud de sortie soient hostiles est ~ `f_entry ×
f_exit` (cf. `docs/security/threat_model.md` — sous hypothèse de choix
uniforme indépendant ; la réalité dépend des règles de sélection).

La structure actuelle amplifie ce risque : les circuits ont une durée
de vie courte, le watchdog les reconstruit à chaque tick de 5 s dès
que le stock passe sous `min_circuits`, et chaque `create` choisit un
premier saut dans tout le pool (`first_hop_candidates`). Un adversaire
qui force des reconstructions (storm de `DESTROY`, relais instable)
multiplie les tirages d'entrée — chacun est une chance supplémentaire
de tomber sur un entry hostile qui voit l'IP réelle.

Tor résout ça avec les *guard nodes* : un petit ensemble de premiers
sauts persistants, réutilisés pendant des semaines. Un tirage hostile
n'arrive qu'une fois par rotation de guards au lieu d'une fois par
circuit. pyipv8 **n'a pas** ce mécanisme — c'est un écart assumé à
documenter (cf. règle « fidélité protocole » : écart motivé par la
sécurité, tracé ici).

## Décisions à trancher

### 1. Taille du set

Recommandation : **3 guards actifs + 2 de réserve** (comme le `NumEntryGuards=3` historique de Tor). Un guard unique concentre
tout le trafic sur un point d'observation ; 3 répartit sans diluer la
persistance. La réserve absorbe les morts sans tirage immédiat.

### 2. Durée de persistance

Recommandation : **30 jours par guard** (entre le `GuardLifetime` Tor
de ~4 mois et l'absence totale de persistance pyipv8), borné par la
durée de vie du profil utilisateur. Un guard mort ou inaccessible
depuis > 24 h est rétrogradé en réserve avant expiration.

### 3. Rotation

Déclencheurs : expiration naturelle, échec prolongé (handshake `create`
en timeout répété), compromission présumée (comportement anormal :
cellules malformées, destroys ciblés), reset utilisateur explicite.
La rotation remplace par le prochain guard de réserve, puis reteste
l'ancien en tâche de fond — jamais de tirage synchrone sous pression.

### 4. Stockage

`onionbit-tunnel` **ne dépend pas** de `onionbit-db` (sens des dépendances
: `db → core → tunnel`) — la couture est un trait injecté :

```rust
// dans onionbit-tunnel
trait GuardStore: Send + Sync {
    fn load_guards(&self) -> Vec<GuardRecord>;
    fn save_guards(&self, guards: &[GuardRecord]);
}
```

- `GuardStore` implémenté dans `onionbit-db` (table `guards` : clé
  publique, dernière adresse vue, date d'adoption, compteur d'échecs,
  statut actif/réserve), injecté par `core`/`daemon` à la construction
  de la communauté ;
- sans store injecté (tests, outils) : `InMemoryGuardStore` — set
  volatile, comportement identique sauf persistance ;
- pas de chiffrement supplémentaire — la base n'est pas chiffrée
  ailleurs ; le contenu est des clés publiques, non secrètes au sens
  crypto (mais révélatrices de la topologie d'usage si le disque est
  lu — cf. threat model : compromission d'endpoint hors périmètre).

### 5. Diversité

Sélection des guards en évitant, dans l'ordre de faisabilité : même
adresse IP (trivial), même /24 (simple), même AS (nécessite une base
ASN — **reporté**, dépendance lourde pour le gain). Le repli sur
diversité impossible (maillage trop petit) logue un WARN.

### 6. Bootstrap

Au premier démarrage (aucun guard connu) : tirer 3+2 candidats dans le
pool existant (`first_hop_candidates` actuel = critères inchangés,
juste un tirage initial plus grand). Risque assumé : les premiers
guards sont un tirage comme les autres — la protection commence au
second circuit. Pas de liste de guards « officiels » codée en dur
(centralisation + point de compromission).

### 7. Compatibilité pyipv8

Écart filaire : **nul** — les guards changent uniquement la *sélection*
locale du premier saut ; aucun champ de protocole ne change. L'écart
est comportemental (réutilisation d'entrées), compatible avec les
relais Tribler existants qui ne savent pas qu'ils servent de guard.
Fallback : `--no-guards` / setting `tunnel.community.guards_enabled`
pour retrouver le comportement pyipv8 exact (diagnostic, interop).

### 8. UX

La liste des guards n'est **pas** exposée dans l'UI par défaut
(risque de confusion) ; un endpoint `/api/ipv8/tunnel/guards` lecture
seule + `DELETE` pour reset est suffisant pour le diagnostic. Un
avertissement UI n'est pas requis — les guards sont un durcissement
transparent.

### 9. Reconstruction

`build_circuits_if_needed`/`create_circuit_inner` consomment d'abord
les guards actifs vivants ; le tirage aléatoire n'intervient qu'en
bootstrap ou épuisement. Un `DESTROY` storm sur les guards ne peut
donc **pas** forcer un tirage d'entrée — c'est la propriété de sécurité
centrale, à épingler par test (`destroy_storm` étendu : les destroys
ne multiplient pas les premiers hops distincts).

### 10. Mode direct

Jamais de guard logic sur `hops == 0` (lane publique) — la sélection
public n'existe pas (`anon_engine` rejette 0). Guards uniquement pour
les circuits des lanes anonymes (`DATA`, `IP_*`, `RP_*`).

## Questions ouvertes résolues

### Pool réduit vs réseau public

Le set de guards n'est jamais une liste codée en dur ni un serveur
« officiel » : les guards sont tirés du pool `first_hop_candidates`
existant (critères de relais inchangés). Règles de dégradation :

- pool < 5 candidats au bootstrap : activer `min(pool, 3)` guards,
  pas de réserve, WARN `guards_pool_etroit` ;
- un guard unique vivant est toujours préférable au tirage libre —
  le plancher n'est jamais « retour au hasard silencieux » ;
- pool vide : comportement pyipv8 inchangé (pas de circuit, le
  watchdog retente).

### Comportement sous `DESTROY` répétés

Propriété centrale : la reconstruction consomme le guard existant —
un storm de `DESTROY` sur les circuits d'un guard sain produit des
circuits **sur le même premier saut** (test : les premiers hops
distincts pendant un storm ⊆ set de guards). Le guard n'est jamais
retiré pour cause de destroys entrants — seul un échec de handshake
`create` compte (cf. rotation).

### Persistance et changement d'adresse

Le guard est identifié par sa **clé publique** (`public_key_bin`),
pas par son adresse : un guard qui change d'IP reste le même guard —
l'adresse est rafraîchie à chaque handshake `create` réussi
(`get_verified_by_address` puis réassociation). Un pair qui change de
clé publique est un nouveau candidat : l'ancien guard expire par
timeout d'injoignabilité (24 h).

### Limites de diversité

Dédup à l'admission : même adresse IP exclue, même /24 exclu (IPv4) /
même /64 (IPv6). Diversité AS **reportée** (nécessite une base ASN —
gain non démontré sur un maillage de cette taille). Si les contraintes
ne peuvent être satisfaites (maillage homogène), on complète avec le
meilleur candidat restant et on logue `guards_diversite_partielle`.

### Non-régression `guards_enabled=false`

Setting `tunnel.community.guards_enabled` (défaut `true` en anonyme).
`false` → `first_hop_candidates` inchangé, aucune lecture/écriture de
la table `guards` — repli strict pyipv8. Le test de régression compare
la distribution des premiers hops avec/sans guards sur le même pool
et vérifie l'absence totale d'effet de bord en mode `false`.

## Point d'intégration

Les guards s'insèrent en amont de `first_hop_candidates`
(`community.rs:965`) : la liste ordonnée passée à
`send_initial_create` devient `guards_actifs ++ reserve ++
tirage_libre`. Les mécanismes existants (retry sur alternates,
`required_exit` qui épingle le *dernier* saut, `pinned_hops` pour les
sauts intermédiaires) restent inchangés — les guards ne concernent
que l'entrée du circuit.

## Conséquences

- Positif : borne ferme sur les tirages d'entrée ; un attaquant ne
  peut plus multiplier les chances d'observer l'IP réelle en forçant
  des reconstructions.
- Négatif : un guard hostile persistant voit *tout* le trafic entrant
  pendant 30 jours au lieu d'une fraction — le risque se concentre.
  C'est le compromis fondamental des guards (Tor le documente :
  « guard fraction vs guard lifetime »).
- Complexité : nouvelle table DB, politique de sélection modifiée,
  tests de persistance et de rotation à écrire.
- Hors périmètre : le guard ne protège que l'*entrée* — la sortie
  reste un tirage par circuit (les guards de sortie Tor n'existent
  pas non plus).

## Tests attendus à l'implémentation

- Persistance : redémarrage → mêmes guards rechargés depuis la base.
- Storm : destroys en rafale → premiers hops ⊆ set de guards.
- Rotation : guard en échec répété → remplacé par la réserve.
- `guards_enabled=false` → comportement pyipv8 inchangé (régression).
