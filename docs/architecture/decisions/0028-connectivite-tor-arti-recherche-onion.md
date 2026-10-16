# ADR-0028 — Connectivité Tor embarquée (arti) : recherche onion et sortie passerelle

Statut : Proposée (2026-10-16), amendée après revue externe
(2026-10-16) — §1 parseur `httparse`, §5 trait
`OnionSearchBackend` + erreurs explicites, §7 passerelle
requalifiée phase 2 avec contrainte de sauts, §Sécurité étoffée.
Plan d'implantation : `docs/plans/roadmap_adr0028.md`
(étapes 112-118 ; 117 = phase 2).

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
  "enabled": false,
  "mode": "embedded",                    // "embedded" | "external"
  "external_socks5": "socks5h://127.0.0.1:9050",
  "allowed_hosts": [ "<adresse>.onion", "…" ],
  "max_response_bytes": 524288,
  "request_timeout_ms": 30000,
  "bootstrap_timeout_ms": 120000,
  "max_redirects": 3,
  "search": { "backends": { "tpb": { "mirrors": ["<a>.onion", "…"],
      "url_template": "https://{host}/search.php?q={q}" } } },
  "gateway": { "enabled": false, "rate_limit_per_circuit": 10,
               "min_requester_hops": 3 }
}
```

- **Aucune valeur en dur** : seuils, timeouts et adresses vivent dans
  la config (règle AGENTS). L'adresse d'un backend (ex. TPB) est en
  config — les adresses onion changent, la config doit permettre le
  remplacement sans rebuild.
- `enabled` **défaut `false`**, opt-in explicite : le bootstrap Tor
  télécharge un consensus (activité réseau visible du FAI) et Tor est
  bloqué dans certains pays — jamais de démarrage silencieux, et le
  bootstrap ne doit **jamais bloquer le daemon** (runtime en tâche
  de fond ; échec → `state:"error"` diagnostic, daemon opérationnel).
- Les entrées `allowed_hosts` sont validées au chargement :
  suffixe `.onion` + format v3 vérifié par checksum (§4) — une
  entrée invalide est rejetée avec un warn explicite.

### 3. Frontière réseau — `TorTargetPolicy` dans `network-policy`

La seule cible joignable par le fetcher est la **allowlist** :

- hôte suffixe `.onion` de forme v3 valide **et** présent dans
  `allowed_hosts` ; port ∈ {80, 443} ; méthodes `GET`/`HEAD` ;
  réponse ≤ `max_response_bytes`.
- **Jamais** de repli clearnet : Tor injoignable → erreur explicite
  remontée à l'appelant (sémantique kill switch, même discipline
  que `proxy_guard` — pas de fetch hors Tor « pour dépanner »).
- Pas de cible clearnet non plus : le fetcher n'est pas un proxy
  générique — `connect()` n'est exposé qu'à travers
  `TorTargetPolicy::check`.

`onionbit-network-policy` n'est pas affaibli : la politique s'y
ajoute (anti-SSRF étendu aux cibles onion).

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
  - `source` ∈ backends déclarés en config (v1 : `tpb`) — chaque
    backend implémente un **trait `OnionSearchBackend`**
    (`search(query) -> Vec<ExternalResult>`) pour isoler la
    fragilité du scraping et permettre d'autres sources (ahmia…) ;
  - chaque backend déclare une **liste de miroirs** dans
    `tor.search.backends.<id>.mirrors` (adresses onion
    interchangeables) — les `.onion` changent ou tombent : le
    backend bascule sur le miroir suivant avant de rendre erreur ;
  - scraping HTML → résultats normalisés
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
- `GET /api/tor/status` → `{enabled, mode, state, progress}` —
  alimente la page diagnostic et le chip de source.
- **Journalisation** : aucun terme de recherche dans `tracing`
  (niveau info ou plus) — données sensibles ; les événements SSE
  restent derrière `api_key_auth`.
- `apibay.org` (API JSON officielle TPB, utilisée par le plugin
  qBittorrent) est **clearnet-only** : joignable uniquement via un
  noeud de sortie Tor — anonymat dégradé et blocage fréquent des
  exits. Backend optionnel `tpb-api` prévu derrière
  `tor.api_via_exit`, **désactivé** (scraping onion = trafic 100 %
  onion, zéro exit — meilleur anonymat).

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
- **Contrainte de sauts (demandeur)** : tout circuit destiné à
  porter une requête `EXIT_TOR` doit être construit avec **≥ 3
  sauts** (`tor.gateway.min_requester_hops`, défaut 3), quel que
  soit le profil actif — sur 1 saut, la passerelle associerait
  l'IP réelle du demandeur *et* le terme de recherche. Limite
  honnête : une passerelle ne **peut pas vérifier** le nombre de
  sauts (elle ne voit que son hop entrant — propriété de
  l'onion routing elle-même) ; la contrainte est donc une politique
  **côté demandeur**, documentée comme telle — le client refuse
  d'émettre une requête `EXIT_TOR` sur un circuit < 3 sauts.
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

### 8. Hors scope explicite

- **Relais Tor** (guard/middle/exit du réseau Tor) : arti est
  client-only — aucun support relais ; un exit Tor suppose le
  daemon C et une exposition légale. Rejeté.
- **Overlay IPv8 sur Tor** : impossible — Tor ne transporte que
  TCP, l'overlay est UDP.
- **Hébergement `.onion`** (exposer l'API, la messagerie ou des
  partages en service caché) : arti `onion-service` est
  expérimental — phase ultérieure, ADR séparé.
- **Bridges/obfs4** via `tor-ptmgr`, **distribution GetTor**,
  backend `tpb-api` via exit : jalons roadmap, pas la v1.

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
  `state:"error"` remonte à l'UI avec diagnostic clair ; bridges
  = jalon ultérieur (§8).
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
- **`hyper`/`reqwest` complet sur `DataStream`** : rejeté en v1 —
  HTTP/1.1 minimal suffit (GET, réponse bornée) ; à réévaluer si
  HTTPS-on-onion devient nécessaire.
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
- **Limites assumées** : pas de relais Tor, pas d'UDP, scraping
  fragile (mitigé par fixtures), `apibay` non-oignon, pas
  d'onion-service en v1.
