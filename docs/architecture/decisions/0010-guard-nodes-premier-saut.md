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

`onionbit-db` : table `guards` (clé publique, adresse estimée, date
d'adoption, compteur d'échecs, statut actif/réserve). Pas de chiffrement
supplémentaire — la base n'est pas chiffrée ailleurs ; le contenu est
des clés publiques, non secrètes au sens crypto (mais révélatrices de
la topologie d'usage si le disque est lu — à documenter dans le threat
model : compromission d'endpoint déjà hors périmètre).

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
