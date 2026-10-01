# Modele de menace — OnionBit (alpha)

Ce document explicite ce que les bancs de validation **prouvent** et ce
qu'ils **ne prouvent pas**. Il complete `revue_garde_fous.md` (garde-fous
implementes) avec la frontiere de confiance reelle du projet.

## Ce qui est demontre par les bancs

Sur le maillage interop controle (Tribler 8.4.3 officiel + daemons Rust)
et sur le reseau public Tribler :

- **Interop hidden-service bidirectionnelle** : seeder Rust → downloader
  pyipv8 et inversement, a 1, 2 et 3 sauts.
- **Integrite** : verification par pieces en vol + SHA-256 de bout en
  bout sur chaque run.
- **Resilience** : perte du seeder (drain borne, fenetre morte stricte,
  restart + fastresume), perte de l'intro point, perte de l'ancre de
  bootstrap en plein transfert — reconstruction de circuits et reprise
  completes.
- **Attribution DHT** : les annonces d'intro points sont liees au
  `seeder_pk` d'origine, avec `last_seen` post-panne comme preuve de
  fraicheur.
- **Absence de repli direct** : aucun trafic BitTorrent/uTP hors tunnel
  dans les scenarios controles ; kill switch a portees (proxy +
  circuits) prouve en plein transfert.
- **Reseau reel** : telechargement anonyme a 3 sauts via des exits
  Tribler reels (magnet public, octets verifies).
- **Egress borne aux relais** (`hidden_service_egress_uniquement_vers_relais`)
  : pendant `join_swarm`/`peers-request`/`create-e2e`/`link-e2e` et la
  donnee e2e a 2 sauts, le tap `UdpEndpoint` prouve que chaque datagramme
  emis par le downloader est a prefixe tunnel et destine a un premier
  saut de circuit — le point d'introduction et le seeder ne recoivent
  jamais de paquet source de l'adresse reelle du downloader.
- **Pas de scrape clair d'un swarm anonyme**
  (`torrent_checker_never_scrapes_anonymous_infohash`) : un infohash
  `anon_hops > 0` n'est jamais annonce aux trackers depuis l'IP reelle
  — ni par `check_oldest` (rotation SQL) ni par `check_tracker`
  (filtre en amont des chemins API et periodique). Ecart assume avec
  Tribler upstream, dont le health-check sort en clair.

## Ce qui n'est PAS demontre

Les points suivants sont hors perimetre de preuve des bancs actuels.
Un attaquant de cette classe n'est **pas** couvert :

- **Correlation de trafic / adversaire global** : un observateur voyant
  les deux extremites d'un circuit peut correlater timings et volumes.
  Aucun padding, aucune obfuscation temporelle n'est implementee — meme
  constat que Tribler upstream.
- **Sybil massif** : un attaquant controlant une fraction significative
  des relais/exits du maillage peut occuper plusieurs positions du meme
  circuit. Le maillage de test a 3-5 noeuds honnetes ; la resistance au
  denominombrement hostiles n'est ni testee ni bornee.
- **Adversaire actif aux exits** : un exit malveillant voit le trafic
  BitTorrent clair (protocole en clair par construction) et peut
  analyser/modifier les pieces non chiffrees — l'integrite par pieces
  protege l'achemineur, pas l'exit.
- **Fuites applicatives hors banc** : l'audit des points d'egress a
  elimine le scrape clair des swarms anonymes (`torrent_checker`) et
  l'invariant d'egress hidden-service est teste ; mais un chemin de
  code non exerce (ex. un futur endpoint API ou service ajoute) n'est
  garanti que par la revue, pas par mesure. Points residuels connus :
  la resolution DNS des URI `http(s)` utilisateur (`check_uri_policy`)
  et la synchronisation periodique `trackers_file` sortent en clair —
  aucune des deux ne transporte d'infohash.
- **Fingerprinting de l'implementation** : les differences de timing,
  tailles de paquets ou comportements de retry entre Rust et pyipv8
  peuvent rendre un noeud OnionBit identifiable sur le reseau.
- **Disponibilite contre censure du bootstrap** : les bancs supposent
  au moins un point d'amorcage joignable ; un reseau ou tous les
  bootstrapper Tribler sont bloques n'est pas couvert.
- **Endurance long terme** : pas de preuve multi-jours (churn reel,
  rotation de circuits en arriere-plan, pression memoire).

## Limite de topologie de banc

Un maillage a un seul `EXIT_BT` rend toute reconstruction de circuit
DATA impossible quand cet exit est tue — `select_exit` (meme regle que
pyipv8) n'a plus de candidat. Ce n'est pas une casse de resilience :
c'est une impossibilite structurelle partagee avec Tribler. Les bancs
`anchor` deployent donc A2 en second exit pour mesurer quelque chose
de significatif.

## Gate de bootstrap Tribler (informational)

Le snapshot `/ipv8/overlays` de Tribler (`peers`, `tunnel`, `exits`)
converge souvent *apres* le debut du transfert. Dans les bancs il est
desormais reporte en `INFO` : les vrais criteres sont fonctionnels —
download accepte, `RP_DOWNLOADER`/`create-e2e` observes, octets
verifies, integrite finale.
