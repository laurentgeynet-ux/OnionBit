# ADR-0028 — Connectivité Tor embarquée (arti) : recherche onion et sortie passerelle

Statut : Proposée (2026-10-16), amendée après revues externes
(2026-10-16) — §1 parseur `httparse`, §5 trait
`OnionSearchBackend` + erreurs explicites, §7 passerelle
requalifiée phase 2 ; sauts passerelle couplés à `number_hops`
(un seul réglage — décision utilisateur), §Sécurité étoffée ;
§Trajectoire phase 3 ajoutée (identité/messagerie/canaux/
coffre-fort côté service, avec corrections au modèle proposé) ;
§8 UI v1 (HUD, diagnostic, réglages — messagerie différée à la
phase 3 : aucun service caché hébergé avant).
Plan d'implantation : `docs/plans/roadmap_adr0028.md`
(étapes 112-119 ; 117 = phase 2, 119 = phase 3 exploration).

## Contexte

OnionBit est une toile onion **autonome** : overlay IPv8 + circuits
multi-sauts UDP (portage de la TunnelCommunity Tribler). Ce réseau ne
parle **pas** le protocole Tor — ses noeuds de sortie sont des hôtes
clearnet ordinaires : ils ne peuvent ni résoudre ni joindre une
adresse `.onion` (service caché Tor v3 = clé ed25519 en base32).

Or les annuaires de services oignon (ex. le TSV curaté
`greyhat-academy/lists.d` : TPB, Ahmia, DuckDuckGo, ProPublica, Tor
BridgeDB, GetTor…) contiennent des ressources directement utiles à
l'écosystème — en premier lieu **des moteurs de recherche de
torrents**. La recherche OnionBit couvre aujourd'hui deux sources
(ADR-0025) : locale FTS (`/api/metadata/search/local`) et réseau
(`PUT /api/search/remote` sur l'overlay). Une source externe onion —
recherche **sur** Tor, téléchargement **sur** les tunnels OnionBit —
est un complément cohérent qui ne dépend d'aucun serveur propre.

Exigence produit : le logiciel doit être **autonome** — pas de
daemon Tor externe requis. [`arti`](https://arti.torproject.org) est
le client Tor officiel réécrit en Rust (`arti-client`, MIT/Apache-2.0
→ compatible GPL-3.0) : embarquable en crate, maintien protocole
suivi par le Tor Project.

C'est une **extension hors fidélité Tribler** (ADR-0020 §extensions,
ADR-0015) : Tribler n'a aucun équivalent — ni fetch onion, ni flag
de sortie Tor. Les écarts de protocole filaire introduits (nouveau
flag de pair) sont bornés à l'opt-in et ignorés proprement par les
pairs ne les comprenant pas.

## Décision

### 1. Nouvelle crate `onionbit-tor` — propriétaire unique de Tor

Toute connectivité Tor vit dans une crate dédiée
`crates/onionbit-tor`, compilée derrière un **feature flag `tor`**
(`arti-client` est lourd : coût de build et d'emprunte binaire
inacceptables en dépendance obligatoire).

- `TorRuntime` : cycle de vie du `TorClient` arti — bootstrap
  explicite (jamais implicite au démarrage du daemon), mode *dormant*
  entre deux usages (économie réseau/batterie), état exposé
  (`off`/`bootstrapping`/`ready`/`dormant`/`error` + progression %).
- `fetch_onion(url) -> Result<Response, TorError>` : ouvre un
  `DataStream` arti (`connect` supporte nativement les hôtes
  `.onion` — **feature `onion-service-client` requise**, elle tire
  `tor-hsclient`/`tor-hscrypto` ; features recommandées
  `rustls,tokio,onion-service-client` — rustls évite OpenSSL),
  écrit une requête **HTTP/1.1** (`GET`/`HEAD` seulement), lit une
  réponse bornée. Le décodage de la réponse (statut, en-têtes,
  `Transfer-Encoding: chunked`, fins CRLF/LF) passe par
  **`httparse`** (MIT/Apache-2.0, zéro allocation, éprouvé) — un
  parseur maison serait un piège classique. Les redirections
  301/302 sont suivies uniquement si `Location` reste un hôte
  `.onion` de l'allowlist, bornées par `max_redirects` (sites
  miroirs fréquents). **Pas de TLS** : le transport onion est
  chiffré de bout en bout et authentifié — la clé ed25519 du
  service *est* l'adresse. TLS-par-dessus (rares cibles
  HTTPS-on-onion) est hors scope v1.
- Deux modes de transport, une seule façade :
  - `Embedded` : arti embarqué — chemin nominal (autonomie).
  - `External` : `socks5h://127.0.0.1:<port>` d'un Tor système —
    URL validée loopback par
    `network_policy::proxy_guard::validate_local_socks5_url`
    (réutilisée telle quelle), transport via `tokio-socks` (déjà
    présent via le librqbit vendored → promu dépendance directe).
- Le `state_dir` d'arti (consensus, gardes, cache) vit sous
  `state_dir/tor` — contenu de faible sensibilité, cohérent avec
  l'arborescence portable (ADR-0018).

La crate dépend de `onionbit-network-policy` (frontière §3) et
`onionbit-format` (validateur §4) — jamais l'inverse.

### 2. Configuration `tor.*` (section sparse, `daemon_config`)

```json
"tor": {
  "enabled": true,
  "mode": "embedded",                    // "embedded" | "external"
  "external_socks5": "socks5h://127.0.0.1:9050",
  "allowed_hosts": [ "<adresse>.onion", "…" ],
  "max_response_bytes": 524288,
  "request_timeout_ms": 30000,
  "bootstrap_timeout_ms": 120000,
  "max_redirects": 3,
  "search": { "backends": {
      "tpb":    { "mirrors": ["<a>.onion", "…"],
        "url_template": "http://{host}/q.php?q={q}",
        "max_results": 100 } } },
  "gateway": { "enabled": false, "rate_limit_per_circuit": 10 }
}
```

> Pas de `min_requester_hops` séparé : le nombre de sauts des
> circuits passerelle **est** `download_defaults.number_hops` —
> un seul réglage d'anonymat pour tout le logiciel (§7).

- **Aucune valeur en dur** : seuils, timeouts et adresses vivent dans
  la config (règle AGENTS). L'adresse d'un backend (ex. TPB) est en
  config — les adresses onion changent, la config doit permettre le
  remplacement sans rebuild.
- `enabled` **défaut `true`** *(décision 2026-10, amendée)* : la lane
  permanente est un service de fond du produit — la recherche `.onion`
  aujourd'hui, la messagerie/canaux/coffre demain (ADR-0029) — et une
  lane opt-in resterait froide au premier usage. La désactivation
  reste un réglage explicite appliqué à chaud. Rappel affiché en
  réglages : le bootstrap télécharge un consensus (activité réseau
  visible du FAI) et Tor est bloqué dans certains pays ; le bootstrap
  ne doit **jamais bloquer le daemon** (runtime en tâche de fond ;
  échec → `state:"error"` diagnostic, daemon opérationnel).
- **`enabled` = lane permanente** *(décision 2026-10, amendée — voir
  §6 bis roadmap)* : quand `enabled=true`, le bootstrap part **au
  démarrage du daemon** (`Session::new_shell`) — l'activation est le
  geste d'intention. La recherche externe hérite d'une lane déjà
  chaude ; son appel `bootstrap()` n'est plus qu'un filet
  d'auto-réparation sur état `Off`/`Error` (idempotent). La lane
  permanente est requise par la phase 3 (messagerie/canaux/coffre
  `.onion`, ADR-0029) : un service joignable suppose un client
  vivant en continu, pas monté à la demande.
- **Hot-swap sans redémarrage** *(décision 2026-10)* :
  `POST /api/settings` qui modifie la section `tor` permute
  **runtime + backends** atomiquement (`Session::swap_tor_stack`) —
  activation → bootstrap immédiat ; désactivation → l'ancien
  runtime meurt avec le swap (circuits coupés par le drop). Un PUT
  sans section `tor` ne permute rien (pas de rebootstrap parasite).
- Les entrées `allowed_hosts` sont validées au chargement :
  suffixe `.onion` + format v3 vérifié par checksum (§4) — une
  entrée invalide est rejetée avec un warn explicite.

### 3. Frontière réseau — `TorTargetPolicy` dans `network-policy`

La seule cible joignable par le fetcher est la **allowlist** :

- hôte suffixe `.onion` de forme v3 valide **et** présent dans
  `allowed_hosts` ; port ∈ {80, 443} ; méthodes `GET`/`HEAD` ;
  réponse ≤ `max_response_bytes` ;
- **aucun hôte clearnet** : la famille `exit_hosts` /
  `tor.api_via_exit` a été **retirée** (2026-10, décision
  utilisateur — un appel qui traverse un nœud de sortie est vu en
  clair par lui : anonymat dégradé *par construction*, pas un
  compromis acceptable). Un miroir de backend non-`.onion` est
  rejeté au chargement.
- **Jamais** de repli clearnet : Tor injoignable → erreur explicite
  remontée à l'appelant (sémantique kill switch, même discipline
  que `proxy_guard` — pas de fetch hors Tor « pour dépanner »).
- Pas de cible non déclarée non plus : le fetcher n'est pas un
  proxy générique — `connect()` n'est exposé qu'à travers
  `TorTargetPolicy::check`.

`onionbit-network-policy` n'est pas affaibli : la politique s'y
ajoute (anti-SSRF étendu aux cibles onion) — les hôtes de sortie
sont une seconde allowlist explicite, pas une ouverture.

### 4. Validateur d'adresses onion v3 — `onionbit-format::onion`

Module `format::onion` : parse/valide `base32(pubkey ‖ checksum ‖
version)` — 56 chars, `checksum = SHA3-256(".onion checksum" ‖
pubkey ‖ 0x03)[0..2]` (nouvelle dépendance `sha3`, MIT/Apache-2.0).
Vecteurs de test : les ~30 adresses réelles de la liste curatée.
Réutilisé par : validation de `allowed_hosts`, validation des
cibles `TorTargetPolicy`, et plus tard tout parsing de liens
`.onion` dans la messagerie/magnets.

### 5. Source de recherche externe

- `GET /api/search/external?source=tpb&q=…` — sous `api_key_auth` :
  - `source` ∈ backends déclarés en config (`tpb`) — chaque
    backend implémente un **trait `OnionSearchBackend`**
    (`search(query) -> Vec<ExternalResult>`) pour isoler la
    fragilité du scraping et permettre d'autres sources (ahmia…) ;
  - chaque backend déclare une **liste de miroirs** dans
    `tor.search.backends.<id>.mirrors` (adresses onion
    interchangeables) — les `.onion` changent ou tombent : le
    backend bascule sur le miroir suivant avant de rendre erreur ;
  - extraction (JSON `q.php` — l'extraction HTML a vécu et est
    morte avec la refonte TPB) → résultats normalisés
    `{name, infohash, size_bytes, seeds, leechers, category,
    source}` ;
  - validation stricte des résultats : infohash 40-hex ou btih
    base32, borne du nombre de résultats, champs texte traités
    comme non fiables (jamais ré-interprétés côté UI) ;
  - **erreurs explicites** remontées à l'UI —
    `mirror_unreachable`, `captcha_detected` (page PoW/vérif
    anti-DDoS fréquente sur les onion à fort trafic),
    `structure_changed` (selecteurs HTML en échec → probable
    refonte du site), `bootstrap_failed` — jamais un échec muet ;
  - résultats diffusés via la notification SSE
    `remote_query_results` **existante** (même cycle de vie :
    uuid, progression, uuid-based stop) — l'app reçoit une source
    de plus sans nouvelle plomberie.
- `GET /api/search/external?source=tpb&q=…` *(implémenté, étape
  115)* → `200 {uuid, source, results}` (même forme JSON que
  `to_simple_dict` + `source`/`magnet`). Paramètres fautifs → `400`
  (`empty_query`, `unknown_source`) ; indisponibilité Tor → `503`
  (`tor_disabled`, `tor_bootstrapping`, `bootstrap_failed`,
  `tor_not_ready`) ; échecs backend → `502` (`mirror_unreachable`,
  `captcha_detected`, `structure_changed`, `no_results`). Le code
  machine est porté par le champ `error.code` de l'enveloppe
  d'erreur — le message reste libre. La recherche **retente** le
  bootstrap quand le runtime est `Off` ou en `Error` (filet
  d'auto-réparation de la lane permanente — le bootstrap nominal
  part au démarrage/activation) mais n'attend jamais sa fin
  (`bootstrapping` → 503 immédiat ; côté UI la requête est
  **relancée automatiquement** quand la lane passe `ready`) ; la
  notification SSE `remote_query_results` porte `peer =
  "tor:<source>"` et l'uuid partagé.
- `GET /api/tor/status` *(implémenté, étape 115)* → `{enabled,
  mode, external_socks5, state, progress, error, sources}` —
  toujours joignable même bootstrap échoué (prouve que le runtime
  ne bloque pas le daemon) ; alimente la page diagnostic et le
  chip de source.
- `GET /api/tor/probe?source=…` *(implémenté)* → `{source,
  reachable, mirrors:[{host, ok, status, latency_ms, error}]}` —
  sonde de joignabilité : un `GET /` borné par miroir, en
  séquence, sur la lane permanente. L'UI en déduit une pastille de
  santé sur le chip de la source — un onion tombé ou déprécié
  (cas observé : l'onion TPB reclassé « Deprecated » sur les
  annuaires) est signalé **sans qu'une recherche ait été lancée**.
  Sondage strictement à la demande (affichage du chip ou tap
  explicite) — jamais de sondage périodique de fond.
- **Journalisation** : aucun terme de recherche dans `tracing`
  (niveau info ou plus) — données sensibles ; les événements SSE
  restent derrière `api_key_auth`.
- `apibay` *(**retiré** 2026-10)* — l'API JSON officielle de TPB
  (`q.php?q=…`) jointe en clearnet **via un exit Tor** derrière
  `tor.api_via_exit` a existé, désactivée par défaut. Retirée
  entièrement : l'option d'anonymat dégradé ne doit pas exister —
  l'onion TPB sert `q.php` en 100 % onion, le fallback sortie
  n'apportait que du dégradé. Normalisation (conservée par le
  décodeur `qphp` du backend `tpb`) :
  `info_hash` validé par `MagnetLink::parse`, magnet synthétisé
  (`btih` + `dn`, l'API ne publie pas d'URI magnet), entités HTML
  des titres déséchappées, catégories numériques → familles TPB,
  sentinelle `id:"0"`/hash nul → `no_results`.
- **Amendement 2026-10 — `tpb` = `q.php` sur l'onion** : le scraping
  `search.php` est mort pour de bon — `search.php` ne sert plus que
  la coquille JS (`print_search()`), mais l'hôte onion **proxifie
  lui-même `q.php`** (son frontend appelle
  `Get(server + '/q.php?…')`, vérifié en direct : 100 résultats
  JSON). Le backend `tpb` réutilise donc le décodeur `qphp` sur
  ses miroirs `.onion` — transport 100 % onion *et* fiabilité de
  l'API, sans sortie clearnet ; `tpb.rs` (scraper HTML) est
  supprimé. Un `url_template` persisté contenant `search.php` est
  migré vers `q.php` au chargement de la config. Robustesse
  constatée nécessaire : l'onion TPB tombe souvent — les échecs
  de transport/statut injoignable retentent `retries` fois sur le
  même miroir (délai `retry_delay_ms`) avant la bascule ; les
  réponses classées ne retentent pas.

### 6. Interaction avec les profils d'anonymat (ADR-0022)

`tor.*` est **hors preset de profil** — orthogonal à
`legacy`/`full`/`custom` :

- Un profil matérialise l'anonymat *des tunnels OnionBit* (hops,
  safeseeding, stealth) ; Tor ne transporte ici que de la
  **métadonnée** (recherche), jamais le payload BitTorrent — Tor
  ne fait d'ailleurs pas d'UDP, le traffic BT ne pourrait pas y
  passer.
- Le toggle `tor.enabled` fonctionne dans tous les profils.
  Argument de cohérence : en posture claire (hops 0), une recherche
  externe non-Tor fuiterait l'IP et le terme — Tor y est au
  contraire le plus utile.
- Conséquence du couplage des sauts (§7) : la passerelle n'est
  accessible qu'aux profils dont `number_hops ≥ 3` — en profil
  `legacy` (1 saut) elle est indisponible ; la recherche **locale**
  via arti reste, elle, utilisable dans tous les profils (le
  tronçon Tor a ses 3 relais garantis par le protocole).
- L'UI distingue la source (« TPB (Tor) ») de « Réseau »
  (overlay IPv8) — l'utilisateur voit quel canal porte sa requête.

### 7. Sortie passerelle `PEER_FLAG_EXIT_TOR` (extension) — **phase 2**

> Livraison **différée** (roadmap : étape 117 après retour
> d'expérience de la v1, étapes 112-116). La valeur autonome v1
> (arti embarqué) ne dépend pas de cette passerelle ; elle pose
> des risques propres documentés ici avant tout engagement.

Un noeud avec `tor.enabled && tor.gateway.enabled` annonce
`PEER_FLAG_EXIT_TOR = 1<<16` (bits 0-3 = pyipv8, 14 = `EXIT_BACKUP`,
15 = `EXIT_HTTP` — 16 est libre ; flag ignoré proprement par les
pairs non-OnionBit).

- Transport : réutilise les messages `http-request`/`http-response`
  (28/29) et la sémantique `http_tunnel.rs` existante — le
  demandeur envoie une requête brute dont l'hôte cible est
  `.onion` ; la passerelle vérifie la cible contre **sa propre**
  `allowed_hosts`, forward via son arti local, renvoie les chunks
  bornés (mêmes `HTTP_RESPONSE_CHUNK`/sémaphore).
- Garde-fous passerelle : `rate_limit_per_circuit`,
  `max_response_bytes` identique, jamais de cible clearnet, refus
  silencieux (comme `EXIT_HTTP` — pas d'oracle).
- **Sauts — un seul réglage, aucun découplage** : le circuit
  passerelle est construit avec **`number_hops` du profil actif**
  (le même réglage que les téléchargements — pas de paramètre
  séparé qui pourrait diverger). La passerelle n'existe que si
  `number_hops ≥ 3` : en dessous, `via_gateway` est **indisponible**
  (UI grisée + explication, jamais d'élévation silencieuse ni de
  repli sur un circuit plus court). En dessous de 3 sauts, la
  passerelle associerait l'IP réelle du demandeur *et* le terme
  de recherche. Limite honnête : une passerelle ne **peut pas
  vérifier** le nombre de sauts (elle ne voit que son hop
  entrant — propriété de l'onion routing elle-même) ; la
  contrainte est donc une politique **côté demandeur**,
  documentée comme telle.
- **Côté Tor, rien à régler** : le chemin passerelle→service est
  un circuit Tor classique — 3 relais **imposés par le protocole**
  (plus 3 sauts côté service jusqu'au point de rendez-vous pour
  un `.onion`). arti n'expose pas de nombre de sauts : l'anonymat
  du tronçon Tor est invariant, seul le tronçon OnionBit suit le
  réglage utilisateur.
- **Risque honeypot** : n'importe quel attaquant peut annoncer
  `EXIT_TOR` pour collecter les recherches des pairs sans Tor.
  Non éliminable (propriété de tout exit volontaire) — assumée,
  affichée dans l'UI (« la passerelle voit vos recherches »), et
  l'usage passerelle reste un repli explicite quand Tor local
  est indisponible, jamais le chemin par défaut.
- **Modèle de confiance, honnêtement** : la passerelle *voit la
  requête en clair* — comme tout noeud de sortie. Le demandeur
  reste protégé côté origine (sous la contrainte ≥3 sauts, son
  IP n'atteint pas la passerelle), et le tronçon
  passerelle→service est protégé par Tor. C'est du
  **« Tor-over-OnionBit »** : un pair sans Tor installé (mobile,
  poste contraint) garde l'accès aux sources onion via les
  volontaires.

### 8. Interface utilisateur (v1)

L'app Flutter possède déjà chaque emplacement — l'intégration
habille des zones existantes, aucune refonte :

- **Recherche** : nouveau membre `TorrentSource.onion` et chip
  « TPB (Tor) » dans la rangée de filtres existante (`All /
  Local / Network / ≥ 10 seeds` — liste de `ChoiceChip`
  extensible). Table de résultats inchangée, badge `tpb` dans la
  colonne Source ; « Ajouter » réutilise le flux magnet existant
  (aucun démarrage automatique).
- **État Tor dans la recherche** : chip dans la barre —
  `bootstrapping` → spinner + %, `off` → CTA vers les réglages.
- **Privacy HUD** (`privacy_hud.dart`, `anonLaneProvider`, sondage
  10 s partagé) : segment Tor = état de la lane uniquement —
  `enabled=false` → grisé honnête (`off` ≠ `error`), jamais de
  donnée de source `.onion` (la santé TPB reste au chip recherche).
- **Diagnostic** : tuile « Tor » dans l'Overview + détail (mode,
  état, progression bootstrap) alimentés par `/api/tor/status`.
  **Pas d'onglet Tor en navigation principale** — la tuile
  suffit.
- **Réglages** : sous-section « Tor » sous Network & Anonymity —
  toggle (défaut **on**, lane permanente), segmented
  Embedded/External + champ `socks5h://` (loopback validé),
  allowlist en lecture seule. Section `tor` appliquée **à chaud**
  (hot-swap — voir §2) : ni `enabled` ni `mode`/`external_socks5`
  n'exigent de redémarrage.
- **Messagerie : rien en v1.** Afficher `votre_clé.onion` sous
  « Your address » prétendrait une joignabilité inexistante —
  aucun service caché n'est hébergé. Phase 3 : adresse **dérivée
  `tor-hs`** (jamais `ipv8-sign`) + pastille de présence
  overlay/Tor par contact.
- i18n fr/en pour chaque chaîne ; `check_i18n.ps1`,
  `flutter analyze`, `flutter test`.

### 9. Hors scope explicite

- **Relais Tor** (guard/middle/exit du réseau Tor) : arti est
  client-only — aucun support relais ; un exit Tor suppose le
  daemon C et une exposition légale. Rejeté.
- **Overlay IPv8 sur Tor** : impossible — Tor ne transporte que
  TCP, l'overlay est UDP.
- **Hébergement `.onion`** (exposer l'API, la messagerie ou des
  partages en service caché) : arti `onion-service` est
  expérimental — trajectoire phase 3 ci-dessous, ADR dédié.
- **Bridges/obfs4** via `tor-ptmgr`, **distribution GetTor**,
  backend `tpb-api` via exit : jalons roadmap, pas la v1.
- **Plan « côté service »** (hébergement `.onion`, bootstrap via
  Tor, pont Tor-over-OnionBit) : trajectoire documentée ci-dessous
  — phase 3 engagée par l'ADR-0029 (Proposée) ; le pont
  Tor-over-OnionBit reste rejeté par son exigence de séparation.

## Trajectoire — phase 3 « côté service » (exploration, non engagée)

> **Engagée par l'ADR-0029 (Proposée, 2026-10-17)** — cette
> trajectoire devient le projet « adresses `.onion` dans
> Messagerie/Canaux/Coffre », borné par l'exigence « fusion UI,
> transports séparés ». Le texte ci-dessous reste l'analyse de
> fond ; les jalons vivent dans `roadmap_adr0029.md`.
>
> Direction documentée à la demande d'une revue externe
> (2026-10-16) : porter/relayer des sous-systèmes OnionBit *côté
> `.onion`*. Aucun code v1 n'en dépend ; condition première :
> maturité du côté serveur d'arti (`onion-service` /
> `tor-hsservice`, API expérimentale à ce jour) — alternative :
> `mode:"external"` + ControlPort d'un tor C pour héberger.

Doctrine **bimodale** confirmée : Tor porte le signalement, la
métadonnée et la messagerie légère ; l'overlay OnionBit porte le
transport massif (uTP). Tor ne fera jamais d'UDP — cette
répartition n'est pas un compromis, c'est aussi une hygiène pour
le réseau Tor (le trafic P2P asphyxie ses relais bénévoles).

- **Identité (ADR-0016) — la bonne primitive existe déjà** :
  l'affirmation « 1 clé = 2 réseaux » est mathématiquement vraie
  (`ipv8-sign` est ed25519 ; une adresse v3 = `base32(pubkey ‖
  checksum ‖ 0x03)`) mais **refusée telle quelle** : réutiliser la
  clé d'identité comme clé de service rendrait le `.onion`
  *strictement équivalent* à la clé publique IPv8 — pont de
  corrélation inter-plans gratuit. Correctif : dérivation HKDF à
  domaine séparé, ajouter `onionbit/identity/tor-hs/v1` → seed
  ed25519 du service caché. Même phrase BIP39, deux plans non
  liables ; pour les canaux/personas publics, liaison *explicite*
  (le canal signe son adresse `.onion` → adresse
  auto-certifiante) — opt-in, jamais implicite.
- **Messagerie (ADR-0011/0019/0026)** : modèle Briar — chaque
  contact joignable via son service caché, e2e sans serveur ;
  traversée NAT/CGNAT universelle ; anonymat symétrique (3+3
  sauts au point de rendez-vous). Contraintes : un `.onion` n'est
  joignable que **pendant que le pair est en ligne** (les
  circuits d'introduction meurent avec le service — aucune
  persistance façon DHT) → l'asynchronisme reste aux boîtes aux
  lettres store-and-forward (ADR-0026), elles-mêmes exposables
  en `.onion` ; latence d'établissement : secondes — texte OK,
  temps réel exclu ; maintien des intro points = coût
  batterie/données (mobile).
- **Canaux (ADR-0025)** : le catalogue `mdblob` d'un canal peut
  être servi par le créateur en HTTP sur son `.onion` —
  complément au `RemoteSelect` UDP, chemin de survie si
  l'overlay est filtré. Disponibilité : le créateur doit être en
  ligne, sauf miroirs d'abonnés volontaires (opt-in).
- **Coffre-fort (ADR-0018/0027)** : cas d'usage le plus propre —
  accès nomade à son propre explorateur privé via `.onion` +
  *client authorization* v3 (x25519) ; réplication aveugle
  coffre→coffre (le pair héberge du chiffré). **Condition de
  faisabilité à vérifier** : le support de l'autorisation client
  côté arti n'est pas garanti à ce jour.
- **Bootstrap et repli** : un endpoint `.onion` de bootstrap
  IPv8 donne un chemin d'amorçage sous censure FAI ; un canal Tor
  peut porter la signalisation entre pairs (négociation de
  rendez-vous). Nuance à la proposition originale : la
  signalisation ne **crée** pas d'accessibilité UDP là où UDP
  est bloqué — elle n'aide que face au NAT, pas au filtrage.
- **Pont furtif vers Tor (Tor-over-OnionBit)** : porter le
  *protocole* Tor (liaison TLS client→garde) dans un tunnel
  OnionBit a un modèle de confiance **meilleur** que `EXIT_TOR` :
  la passerelle ne voit que du chiffré vers une IP de garde —
  pas les recherches en clair. Mais `http_tunnel` est
  requête/réponse, pas un flux bidirectionnel : il faudrait un
  transport de flux sur cellules (chantier dédié), et la
  passerelle devient de facto un bridge Tor (elles restent
  bénévoles — même posture de confiance documentée qu'§7).

## Sécurité / vie privée

- **Corrélation recherche↔swarm** : un adversaire observant à la
  fois le service onion et l'overlay OnionBit peut relier par
  timing/contenu une requête à un join de swarm. Mitigations v1 :
  pas de démarrage de téléchargement automatique depuis un
  résultat externe, jitter appliqué aux requêtes passerelle.
  Limite assumée et documentée (THREAT-MODEL à amender).
- **Honeypot `EXIT_TOR`** : cf. §7 — collecte des recherches par
  passerelles malveillantes ; atténuée par la contrainte de sauts
  demandeur et l'affichage UI, non éliminable.
- **Bootstrap Tor bloqué** (FAI/censure — autorités d'annuaire et
  gardes publics filtrés) : le runtime reste non-bloquant,
  `state:"error"` remonte à l'UI avec diagnostic clair.
  **Ponts vanilla implémentés** (`tor.bridges` — lignes « bridge »
  persistées, parsées par `BridgeConfigBuilder` d'arti, feature
  `bridge-client`) : une liste non vide impose le bootstrap **ponts
  uniquement** (`enabled: Explicit(true)`, aucun repli silencieux
  sur les gardes publics — si aucune ligne n'est valide, erreur
  nette plutôt que contournement de l'intention). Limite assumée :
  un pont vanilla cache l'IP des gardes publics mais pas le
  protocole Tor au DPI — les transports obfusqués (`obfs4`,
  `snowflake` via `pt-client` + binaires externes packagés) restent
  un chantier dédié (§9).
- **Fragilité opérationnelle du scraping** : protections
  anti-DDoS (PoW, pages de vérification), refontes HTML, miroirs
  mouvants — traitées par les erreurs explicites + bascule de
  miroirs (§5), pas par du code défensif opaque.
- **Requêtes jamais loguées** ; pas de réponse brute dans les
  logs ; SSE derrière la clé API (règle secrets étendue aux
  métadonnées sensibles).
- **Parseur de résultats** : HTML non fiable — parseur borné
  (`scraper`, ISC), fixtures figées, champs traités en texte pur,
  compte et taille plafonnés.
- **SSRF** : la seule surface de fetch réseau ajoutée est bornée
  par `TorTargetPolicy` — ni clearnet, ni onion non déclaré.
- **Redirections/cookies** : non suivies / non stockées en v1 —
  le fetcher est un client GET minimal, pas un navigateur.
- `state_dir/tor` : consensus + clés de garde — sensibilité
  faible ; cohérent avec le modèle `state_dir` existant (pas de
  secret d'identité OnionBit dedans).
- **Phase 3 anticipée** : la dérivation `tor-hs` (domaine HKDF
  séparé) est la condition anti-corrélation — jamais la clé
  `ipv8-sign` en service caché direct ; la disponibilité d'un
  `.onion` auto-hébergé = être en ligne (pas de persistance) ;
  le pont Tor-over-OnionBit change le modèle de confiance de la
  passerelle (chiffré, pas clair) sans supprimer le statut de
  bénévole.

## Alternatives considérées

- **Tor externe seul** (SOCKS5 9050/9150) : rejeté comme unique
  option — exige un Tor installé, contraire à l'autonomie ; retenu
  comme `mode:"external"`.
- **`apibay.org` en clearnet direct** : rejeté — fuite d'IP et du
  terme de recherche, contre l'objet même de la fonctionnalité.
- **TPB clearnet via une sortie `EXIT_HTTP` existante** : rejeté —
  l'exit ne peut pas joindre `.onion` de toute façon, et sortir du
  TPB clearnet exposerait l'opérateur de sortie (bénévole).
- **Daemon `tor` C embarqué** : rejeté — binaire non-Rust lourd,
  surface C, distribution multi-plateforme compliquée.
- **`hyper`/`reqwest` complet sur `DataStream`** : rejeté —
  HTTP/1.1 minimal suffit (GET, réponse bornée) ; le besoin TLS
  apparu avec `api_via_exit` est couvert par `tokio-rustls`
  au-dessus du même codec minimal, pas par une pile HTTP
  complète.
- **DHT/annuaire de résultats interne** : la recherche réseau
  native (ADR-0025) existe déjà — la source onion est un
  complément, pas un remplacement.

## Conséquences

- **Positif** : recherche externe anonyme et autonome ; la
  passerelle apporte une valeur de réseau (pairs contraints) ;
  réutilisation maximale de l'existant (`http_tunnel`, SSE
  `remote_query_results`, DTO résultats, `proxy_guard`) ; arti suit
  officiellement le protocole Tor.
- **Vigilance** : poids de build d'arti (atténué par le feature
  gate `tor`) ; confiance passerelle documentée ; adresses onion
  périssables (en config, pas en dur) ; contenu indexé par TPB =
  posture d'outil, opt-in explicite, sources affichées.
- **Limites assumées** : pas de relais Tor, pas d'UDP, source
  externe dépendante de la dispo de l'onion TPB (mitigée par
  `retries`/miroirs), **aucune sortie clearnet** — l'onion mort =
  recherche externe indisponible jusqu'à son retour ; pas
  d'onion-service en v1 (phase 3
  exploratoire documentée : identité dérivée HKDF,
  messagerie/canaux/coffre-fort `.onion`, bootstrap, pont
  Tor-over-OnionBit — chacun sous ADR dédié et maturité arti
  serveur).

## Notes de mise en œuvre

- **`saturating-time` vendored (correctif Windows)** : sans lui,
  tout bootstrap arti boucle indéfiniment sous Windows (parse du
  consensus à 100 % CPU, statut figé à 15 %) — bug upstream
  confirmé (arti #2678, #2726) en cours de correction (#2728).
  `vendor/saturating-time` + `[patch.crates-io]`, détail et
  condition de retrait dans `docs/plans/roadmap_adr0028.md`
  (section « Correctif plateforme »).
