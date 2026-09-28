# ADR-0006 — Écarts résiduels de parité REST/SSE avec Tribler 8.x

Statut : Acceptée (2026-09-28).

## Contexte

La phase 5b (étapes 21-28 du roadmap) porte l'API de contrôle de
Tribler Python dans le daemon Rust avec pour objectif la parité des
shapes, codes de statut, authentification et comportements. Certains
endpoints ou comportements n'ont pas d'équivalent exact en Rust/tokio,
ou ont volontairement été écartés. Cette ADR liste les divergences
résiduelles actées pour que le banc `scripts/api_parity.ps1` et les
travaux futurs sachent les distinguer des bugs.

## Décisions

### `identity/*` non implémenté (exclusion)

Les endpoints `/api/ipv8/identity/*` (pseudonymes, attestations,
vérifications) portent sur la couche identité IPv8, absente du
périmètre V1 du portage (`tribler-ipv8` couvre discovery, content
discovery, DHT, tunnel — pas `IdentityCommunity`). Aucun appelant
daemon/UI de la V1 ne les consomme. **Décision** : exclusion
permanente de la phase 5b ; à réévaluer si une attestation est un
jour requise.

### `/api/ipv8/asyncio/*` adapté à tokio, pas introspectif

Tokio ne fournit pas `all_tasks()`/`Task.get_stack()`/`Task.get_name()`
— l'introspection des tâches asyncio n'a pas d'équivalent.

- `GET /tasks` : la liste provient d'un `TaskRegistry` alimenté par
  les points de spawn nommés (maintenance DHT, watchdogs de circuits,
  progress loop, watchers RSS, torrent checker). `running` est toujours
  `false` et `stack` toujours `[]` — l'introspection de pile n'existe
  pas en Rust.
- `GET/PUT /debug` : `enable` bascule la capture `tracing` vers un
  buffer borné (50) et recharge l'`EnvFilter` global (`debug` ↔
  directive d'origine) ; `slow_callback_duration` est stockée et
  exposée mais sans instrumentation équivalente à
  `loop.slow_callback_duration` d'asyncio.
- `GET/PUT /drift` : fidèle — tache `interval(walker_interval)` mesurant
  `max(0, réel - attendu)`, historique 100, codes d'erreur identiques.

### Speed-test de circuits : double format filaire

`ipv8-rust-tunnels` (backend réel de Tribler 8.x) utilise les cellules
`test-request`/`test-response` **21/22** avec `identifier` u32 ; le
backend Python pur utilise **19/20** avec u16. `tribler-tunnel`
décode et répond aux deux formats et émet en 21/22 — l'interop avec
un pair Python-pur en tant que demandeur fonctionne, en tant que
répondeur seul le format 21/22 est émis (identique au comportement du
backend Rust de Tribler).

### `GET /api/rss` est une extension Rust

Python n'expose que `PUT /api/rss`. Le `GET` (listing `{items}` des
entrées `.torrent` découvertes, table `rss_items` — migration v5) est
un ajout pour l'UI ; il n'a pas de contrepartie à comparer.

### Écarts de valeurs hérités des étapes précédentes

- `GET /api/downloads[].eta` : chaîne formatée (`"3m 20s"`) au lieu
  du float de secondes Python ; `num_seeds`/`num_connected_seeds`
  reflètent le dernier scrape du torrent checker (pas de scrape
  synchrone au listing).
- `file_priority`/`queue_position`/`auto_managed` sont persistés et
  restitués mais sans effet moteur (rqbit n'ordonne pas une file de
  téléchargements ni des priorités de fichiers individuelles).
- `PUT /api/statistics/dirspace` est `GET ?path=` côté Rust ;
  `?hop=` libtorrent est `?session=`.

### Quirks Python reproduits volontairement

Pour rester fidèles au comportement observé (pas de middleware de
validation aiohttp_apispec — les schémas sont docs-only) :

- Query params arrivant en chaînes : `request_size`/`response_size`
  sur `circuits/test` → 500 `{"error":{"handled":false}}`
  (`TypeError` Python) ; `?hops=` sur `swarms/{ih}/size` → `swarm_size`
  0 silencieux.
- `PUT /drift` sans `enable` → 400 `{"error": "incorrect parameters"}`
  **sans** clé `success` ; `PUT /debug` → 400 **avec**
  `{"success": false, "error": "incorrect parameters"}` — les deux
  shapes Python distinctes sont conservées.

## Conséquences

- `scripts/api_parity.ps1` peut rapporter des diffs de shape attendus
  sur `/api/ipv8/asyncio/tasks` (champs `running`/`stack`) —
  considérer ces écarts comme documentés.
- Toute nouvelle divergence constatée doit rejoindre cette liste ou
  être corrigée — pas de divergence silencieuse.
