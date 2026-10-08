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
- **Le point de rendez-vous ne voit pas le downloader meme a
  `hops=1`** (`hidden_service_e2e_roundtrip`, assertion
  `RP_DOWNLOADER.goal_hops == 2`) : `swarm_circuit_hops` ajoute un saut
  aux jambes `IP_SEEDER`/`RP_DOWNLOADER` (parite pyipv8). Le RP —
  choisi par le seeder, potentiellement hostile — ne recoit que le
  premier saut du downloader, jamais son adresse. La regression serait
  detectee par le test.

## Semantique des sauts (rappel)

- `anon_hops = 0` : telechargement **public** (lane non anonyme), par
  design — pas une erreur.
- `anon_hops = 1` : anonymat **reduit** — circuits DATA a 1 saut : le
  noeud de sortie voit l'IP du client (sans savoir qu'il s'agit d'un
  swarm cache). Les jambes e2e restent a 2 sauts (`+1`). Valeur par
  defaut `download_defaults/number_hops = 1`, parite Tribler ;
  preferer 2-3 pour la confidentialite.
- `join_swarm(0, seeding = false)` : impossible de creer un circuit de
  0 saut — les requetes echouent en mode ferme (WARN loguee, parite
  pyipv8 qui ne valide pas ce parametre).
- **Injection de messages e2e non signes**
  (`hidden_service_injection_paquets_forjes`) : `ezr_pack(sig=False)`
  est une parite pyipv8 — la defense repose sur les caches et tables
  de relais, pas sur la signature. Demontre sur socket brute :
  `peers-response`/`created-e2e` a `identifier` sans requete en cours
  rejetes (`peers_requests`/`e2e_requests`) ; `linked-e2e` brut non
  dispatchable hors cellule ; `create-e2e` a `node_public_key` inconnue
  non relaye (`intro_point_for`) ; doublon `create-e2e` rejoue depuis
  `seen_e2e` sans recreer de `RP_SEEDER`. **Residu assume** : un
  `create-e2e` a identifiant neuf avec la vraie `seeder_pk` coute un
  circuit RP au seeder — surface de DoS inherente a la parite pyipv8
  (bornee par la duree de vie des circuits, pas par quota).
- **Robustesse des parsers** (`fuzz_regression`, proptest + vecteurs
  de bord) : aucun parser expose aux pairs hostiles (enveloppe IPv8,
  cellule, payloads e2e/DHT, header uTP) ne panique — entrees
  arbitraires, frontieres de format, compteurs de listes adversaires.
  A trouve et corrige un crash reel : une cellule dechiffrant a
  pile 29 octets faisait paniquer `check_cell_flags` (DoS injectable
  par un relais du circuit). Scaffold `fuzz/` cargo-fuzz pret pour le
  fuzzing couverture-guidee prolonge (nightly + clang requis).
- **Storm de DESTROY borne** (`destroy_storm_et_reconstruction_bornee`)
  : le `DESTROY` n'est authentifie ni par saut ni par pair (parite
  pyipv8 — un premier saut peut toujours abattre son circuit). Epingle
  : nettoyage idempotent, purge symetrique relais/sortie,
  reconstruction possible — et la borne d'amplification
  (`build_circuits_if_needed` : 1 circuit par appel, plafond
  `ready + pending >= min`, rythme du watchdog 5 s cote core).
- **Pas de resolution DNS cote client** (epingle
  `adresse_domaine_ne_se_resout_pas_cote_client`) : `ATYP_DOMAIN`
  SOCKS5 -> `UdpAddress::Domain` opaque, `to_socket_addr` renvoie
  `None` — le domaine est forwarde dans la cellule (`http-request`)
  et resolu par le noeud de sortie (`send_tcp_request`), comme un
  exit Tor. **Residu documente** : bootstrap configure et
  `check_uri_policy` resolvent en clair (amorcage et URI utilisateur).

## Messagerie e2e (ADR-0011, etape 41)

Proprietes **demontrees** par les bancs `MS-*` (loopback 2 demons,
tests hostiles, persistance, API) :

- **Authenticite par trame** (MS-3) : signature Ed25519 de l'emetteur
  verifiee contre la cle publique du contact ; trame signee par une
  autre cle, `sig` tronquee ou corps modifie post-signature → rejet
  systematique, zero ecriture.
- **Anti-replay et ordre** (MS-5) : compteur `seq` + fenetre glissante
  de 64 + dedup borne sur `id` ; les rejoues sont droppes sans
  reponse, le desordre en fenetre est livre une fois ; la dedup ne
  consomme pas de budget (reemission honnete absorbee a la
  reouverture de circuit, MS-2).
- **Codec borne** (MS-4) : trame bencode deterministe `{v,type,id,
  seq,ts,body,sig}` ≤ 32 Kio, `body` ≤ 30 Kio, `v != 1` et champs
  inconnus rejetes ; prefiltre taille+version avant tout parse ;
  cible fuzz `messaging_frame` + miroir stable — zero panic.
- **Consentement borne** (MS-6) : premier `hello` verifie →
  `pending` (capacite + TTL), decision explicite
  accept/refuse/block ; `blocked` → trames ignorees + circuits
  detruits ; non-`hello` d'inconnu droppé avant livraison.
- **Anti-DoS applicatif** (MS-10) : seaux a jetons par contact et
  global ; les drops sont comptes par cause (`stats_snapshot`).
- **Separation de lane** (MS-9) : les swarms `messaging_hash(pk)`
  n'entrent jamais dans `swarm_lookup` ; aucune trame `v=1` ne passe
  `could_be_utp` ; les trames n'empruntent que des cellules `data`
  sur circuits e2e — zero datagramme direct (invariant egress
  inchange).
- **Livraison bornee** (MS-7) : contact hors ligne → erreur
  `Undeliverable` + ligne `failed` visible ; jamais de file ni de
  reemission.
- **Persistance et effacement** (MS-11) : contacts/messages
  restaures au restart (`seq` + `recv_top` repris en conservateur),
  `DELETE` physique, retention optionnelle avec zeroisation du
  corps (`secure_delete`).
- **API** (MS-12) : extension `/api/messaging/*` derriere la cle
  API (401 sans cle), 404 quand le service est desactive.

### Ce que la messagerie ne prouve PAS

- **Metadonnee de presence** : rejoindre le swarm
  `messaging_hash(pk)` publie des annonces DHT periodiques
  d'intro points (`announce_interval`, defaut 300 s). Un observateur
  DHT apprend qu'une identite `pk` utilise la messagerie et quand —
  c'est une metadonnee **assume et mesuree** (MS-8, delta
  consigne dans `fingerprinting.md`), pas dissimulee.
- **Intro point de l'expediteur privilegie** : le point
  d'introduction choisi par le destinataire voit les `create-e2e`
  de ses correspondants (parite hidden services — le contact reste
  derriere `hops` sauts, l'IP de l'expediteur n'est pas exposee).
- **Pas de forward secrecy** : v1 derive les cles applicatives par
  HKDF du secret e2e du circuit, sans ratchet. La compromission
  ulterieure de la cle d'identite ne permet pas de rejouer les
  trames (signatures), mais un secret e2e capture pendant le
  handshake + les trames enregistrees rendent le contenu
  dechiffrable — le ratchet est un travail futur explicite.
- **Persistance en clair** : `msg_messages.body` et les contacts
  sont stockes en clair dans `onionbit.db` (assume dans l'ADR ;
  `secure_delete` zeroise a l'expiration de retention seulement).
  Le chiffrement du volume d'etat n'est pas couvert.
- **Identite partagee** : la cle messagerie est la cle d'identite
  IPv8 du demon — les contacts messagerie peuvent correlater
  l'identite de presence avec les autres usages IPv8 du noeud
  (deliberer pour v1 : un seul trousseau).
- **Correlation de trafic** : les non-claims globaux ci-dessous
  s'appliquent integralement a la messagerie (pas de padding, pas
  de resistance a un adversaire observant les deux extremites).

## Transport furtif (ADR-0017, etapes 49-55)

Banc `scripts/bench_stealth_fingerprint.ps1` (daemon reel pont +
client en loopback via relais consignant PCAP, `stealth_bench`) :

- **Silence au probing** : 1200 sondes calibrees (tailles
  `hs1`/trame, garbage, look-alikes IPv8/DNS/BitTorrent, rejeu de
  datagrammes captures) → 0 reponse sur les deux roles ; rejet
  uniforme quelle que soit la cause (MAC, timestamp, replay,
  saturation) — pas d'oracle de probing differenciable.
- **Amplification ≤ 1** : 1,68 Mo de sondes → 0 octet renvoye ;
  aucune reponse pre-handshake n'est jamais emise vers une adresse
  qui n'a pas complete `hs1`.
- **Zero marqueur legacy sur le fil** : ni `LibNaCLPK:`, ni
  community_id, ni `d1:` bencode, ni prefixe IPv8 dans 363
  datagrammes captures ; entropie 7,99 bits/octet, zero constante
  entre runs, zero datagramme duplique.
- **Resilience** : session maintenue sous 5 % de perte/dup/
  reordonnancement injectes au relais ; re-etablie apres restart
  du pont (`dial_cooldown` borne le motif de retries) et apres
  changement de port source du client ; survive a une rafale de
  4000 sondes. Jamais de repli en clair (fail-closed : tout ce qui
  n'est pas stealth est drope avant le socket).
- **Scraping borne** (`ext::INTRO`, etape 54) : introductions
  servees a quotas gradues (seed budget sans reputation →
  expansion gatee ledger → push large reserve aux reciproques) ;
  les `bridge_pk` annoncees doivent etre prouvees au handshake —
  une intro forgee est inerte.

## Identite portable (ADR-0016, etapes 48a-48e)

- **Vol du `state_dir`** : en mode par defaut la graine
  `identity_seed.bin` est en clair (comme tout wallet « hot ») —
  `0600` Unix mais lisible par un attaquant ayant la session.
  Mitigation opt-in : `identity.at_rest` → graine scellee `OBSK`
  (argon2id 19 Mio → ChaCha20-Poly1305), boot en `locked`,
  `unlock` rate-limite par IP + globalement, erreur uniforme sans
  oracle. Le unlock ne reecrit jamais de clef en clair sur disque.
- **Phrase BIP39 = compromission totale** : les 24 mots derivent
  l'identite IPv8 **et** la cle de pont stealth — quiconque la
  detient EST l'identite. L'endpoint `recovery_phrase` reste
  derriere `api_key_auth`, trace `warn` sans jamais la logguer.
- **Premier boot sous gate** (`--first-run-gate`, lanceur UI) :
  `identity_pending` — aucun composant identitaire ne demarre,
  aucun datagramme signe n'existe avant le choix utilisateur ;
  les endpoints identitaires recoivent `409` uniforme.
- **Session invitee** : identite purement memoire, base `:memory:`,
  zero artefact dans `state_dir` — rien a voler, mais aucune
  continuite (reputation ADR-0015, contacts, coffre).
- **`at_rest` refuse aux ponts/passerelles** : un serveur doit
  redemarrer sans surveillance (fail-closed).

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
  Procedure de mesure : `docs/security/fingerprinting.md` +
  `scripts/fingerprint_stats.ps1` (statistiques agregees REST,
  comparables avec Tribler officiel).
- **Disponibilite contre censure du bootstrap** : les bancs supposent
  au moins un point d'amorcage joignable ; un reseau ou tous les
  bootstrapper Tribler sont bloques n'est pas couvert.
- **Endurance long terme** : pas de preuve multi-jours (churn reel,
  rotation de circuits en arriere-plan, pression memoire).
- **Censeur classifiant (stealth)** : le banc mesure la separation
  face a des corpus synthetiques (dns/quic/wg/noise/ipv8) — un
  classifieur entraine sur du stealth reel, un adversaire qui
  bloque par volume/timing, l'enumeration des ponts ou le
  throttling UDP global ne sont pas couverts (voir
  `fingerprinting.md` §Mode furtif pour les limites assumees).

## Limite de topologie de banc

Un maillage a un seul `EXIT_BT` rend toute reconstruction de circuit
DATA impossible quand cet exit est tue — `select_exit` (meme regle que
pyipv8) n'a plus de candidat. Ce n'est pas une casse de resilience :
c'est une impossibilite structurelle partagee avec Tribler. Les bancs
`anchor` deployent donc A2 en second exit pour mesurer quelque chose
de significatif.

## Declaration de couverture

OnionBit dispose de tests de regression pour les vecteurs suivants :
egress direct hors relais, configuration a faible nombre de sauts,
resolution DNS locale, reconstruction de circuit forcee (storm
`DESTROY`), injection de messages e2e non signes, et entrees de
parseurs hostiles — ces dernieres consolidées par une campagne
libFuzzer coverage-guidee de ~5 h (6,28 Md d'executions, 0 crash,
`docs/security/fuzz_journal.md` ; sans ASan — une campagne Linux
avec sanitizers reste prevue en complement). Ces tests **ne**
protegent **pas** contre la
correlation de trafic globale, les attaques Sybil a grande echelle,
l'analyse d'intersection a long terme, la compromission de l'endpoint,
ni les vulnerabilites d'implementation futures. Les guard nodes
(ADR-0010) visent precisement le premier de ces residus exploitables :
la multiplication des tirages d'entree. Ils sont **actifs par defaut**
depuis le 2026-10-02 (`tunnel_community/guards_enabled`, desactivable
a chaud via `POST /api/settings`), persistes en base (`DbGuardStore`,
table `guards` — le set survit au redemarrage) et valides sur le
terrain : matrice interop guards, persistance redemarrage sur reseau
reel, download public 2 sauts de 276 Mo avec premiers hops ⊆ set. Ils
restent une mesure de reduction d'exposition Sybil, pas une garantie
d'anonymat.

## Gate de bootstrap Tribler (informational)

Le snapshot `/ipv8/overlays` de Tribler (`peers`, `tunnel`, `exits`)
converge souvent *apres* le debut du transfert. Dans les bancs il est
desormais reporte en `INFO` : les vrais criteres sont fonctionnels —
download accepte, `RP_DOWNLOADER`/`create-e2e` observes, octets
verifies, integrite finale.
