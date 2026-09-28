# Mapping API REST — Tribler Python ↔ `tribler-api` (Rust)

Références : `D:\Projet\Tribler_sources\tribler\src\tribler\core\restapi\`
(`rest_manager.py`, `events_endpoint.py`) et
`core\libtorrent\restapi\downloads_endpoint.py`.

Convention générale :

- Préfixe `/api` identique au Python.
- Erreurs au format `{"error": {"handled": bool, "message": str}}`
  (identique à `rest_manager.py`).
- Bind `127.0.0.1` uniquement (API de controle locale).
- **Authentification par clé API** (identique à `ApiKeyMiddleware` de
  `rest_manager.py`) : chaque requête doit présenter la clé via, par ordre
  de priorité, l'en-tête `X-Api-Key`, le paramètre de requête `?key=` ou le
  cookie `api_key` — même en loopback. Réponse en cas d'échec :
  `401 {"error": {"handled": true, "message": "Unauthorized access"}}`.
  La clé est générée hex (32 caractères) au premier démarrage et persistée
  dans `state_dir/configuration.json` (`api/key`). Aucune route n'est
  exemptée (`/docs` et `/ui` n'existent pas côté Rust).
- Corps de requête maximal : **16 Mio** (`MAX_REQUEST_SIZE` Python).
- Configuration persistée : `state_dir/configuration.json` (arbre
  `TriblerConfig`), `api/http_port_running` réécrit après le bind réel.

## Endpoints implémentés

| Python | Rust | Statut | Notes |
| :--- | :--- | :--- | :--- |
| **Downloads** (`downloads_endpoint.py`) | | | |
| `GET /api/downloads` | `GET /api/downloads` | ✅ | `infohash`/`excluded` (filtres) ; `get_peers` (liste `peers` — sous-ensemble rqbit, autres clés aux défauts), `get_pieces` (bitfield base64 MSB-first), `get_availability` (`float(num_seeds)` scrapé — librqbit ne fusionne pas les bitfields pairs, approximation documentée) ; actifs seulement quand le paramètre vaut `"1"`, comme Python |
| `PUT /api/downloads` | `PUT /api/downloads` | ✅ | `uri` (magnet/http) et `torrent` (chemin local) ; `destination`, `paused` acceptés ; `anon_hops` routé vers la lane anonyme du stack IPv8 (étape 15) |
| `DELETE /api/downloads/{infohash}` | `DELETE /api/downloads/{infohash}` | ✅ | `remove_data` supporté |
| `PATCH /api/downloads/{infohash}` | `PATCH /api/downloads/{infohash}` | ✅ | Sémantique Python complète : exclusivité `anon_hops` (comptage brut des clés), `selected_files` (`Session::update_only_files`), `file_priority` (persistée 0..=7 — pas d'ordonnancement rqbit), `upload_limit`/`download_limit` (persistées, appliquées au re-add via `initial ratelimits` — pas de mutation à chaud par torrent dans rqbit), `seeding_ratio`/`seeding_ratio_default`, `queue_position` (ordre persisté — librqbit n'a pas de file lt), `auto_managed` (persisté), `state` = `resume`/`stop` (idempotents, `user_stopped` persisté), `recheck` (remove + re-add → revalidation complète), `move_storage` (déplacement disque + re-add ; no-op → `modified:false`, dossier absent → 400, `dest_dir` manquant → 500 `KeyError` handled) |
| `GET /api/downloads/{ih}/torrent` | `GET /api/downloads/{ih}/torrent` | ✅ | `.torrent` brut, `Content-Type: application/x-bittorrent` |
| `GET /api/downloads/{ih}/trackers` | idem | ✅ | `{"tracker_info": [{"url"}]}` — announce/announce-list + ajouts à chaud |
| `PUT /api/downloads/{ih}/trackers` | idem | ✅ | `{"url"}` → `{"added": true}` ; persisté dans `extra_trackers` (rqbit n'annonce pas un tracker ajouté à chaud — effectif au prochain re-add, divergence documentée) |
| `DELETE /api/downloads/{ih}/trackers` | idem | ✅ | `{"url"}` → `{"removed": true}` ; persisté dans `removed_trackers` et appliqué au re-add en purgeant `announce`/`announce-list`/`tr` de la source (rqbit n'expose pas `replace_trackers` à chaud — le torrent actif continue d'annoncer jusqu'à sa recréation) |
| `PUT /api/downloads/{ih}/default_trackers` | idem | ✅ | → `{"added": true}` ; lit `libtorrent/download_defaults/trackers_file` (format uTorrent : lignes non vides), synchronisé depuis `trackers_file_sync_url` (TTL 1 h, comme `sync_default_trackers_file`). Les nouveaux downloads reçoivent aussi ces trackers automatiquement (post-handle `ADD_DEFAULT_TRACKERS`), sauf torrents `private` |
| `PUT /api/downloads/{ih}/tracker_force_announce` | idem | ✅ | `{"url"}` → `{"forced": true}` même si l'URL ne correspond à rien (quirk Python) ; implémentation = pause+unpause rqbit → réannonce de **tous** les trackers (superset du `force_reannounce(0, i)` ciblé) |
| `GET /api/downloads/{ih}/files` | `GET /api/downloads/{ih}/files` | ✅ | index, name, size, progress par fichier |
| `GET /api/downloads/{ih}/stream/{fileindex}` | idem | ✅ | flux HTTP chunked avec `start` (seek) ; borné par le buffer rqbit |
| `GET /api/events` | `GET /api/events` | ✅ | **SSE** : trames `event: <topic>\ndata: <json>\n\n`, message initial `events_start` |
| `GET /api/events/info` | `GET /api/events/info` | ✅ | `{"public_key", "version", "sessions"}` — `sessions` = nombre de flux SSE ouverts (compteur réel, +1/−1 à la connexion/déconnexion) |
| `GET /api/downloads/clierrors` | `GET /api/downloads/clierrors` | ✅ | File `unhandled_cli_log` : erreurs de `PUT /api/downloads` avec `cli:true`, insertions en tête bornée à 100 ; `GET` vide la file (`{"errors": [...]}`) ; le champ `clierrors` de `GET /api/downloads` = longueur de la file |
| **Settings / shutdown / stats** | | | |
| `GET /api/settings` | `GET /api/settings` | ✅ | Arbre `TriblerConfig` complet lu depuis `configuration.json` persisté ; reflète les overrides à chaud et les réglages inconnus tels qu'écrits |
| `POST /api/settings` | `POST /api/settings` | ✅ | Merge récursif dans la config persistée + réécriture de `configuration.json` (comme `config.write()` Python) ; application à chaud : `rss.urls`, `watch_folder.*`, `libtorrent.*`, `tunnel_community.min/max_circuits`, `api.*`, `ipv8.*` ; le reste est persisté (effet au redémarrage) |
| `PUT /api/shutdown` | `PUT /api/shutdown` | ✅ | Réponse immédiate, arrêt en tâche de fond |
| `GET /api/statistics/tribler` | idem | ✅ | `db_size`, `num_torrents`, `num_channels`, `peers`, `libtorrent.sessions` (lanes anonymes) |
| `GET /api/statistics/ipv8` | idem | ✅ | `total_up`/`total_down` de l'endpoint UDP |
| `PUT /api/statistics/dirspace` (`{"directory"}`) | `PUT` | ✅ | `{"statistics": {total, used, free}}` du premier ancêtre existant du chemin (404 "No stats for directory!"), fidèle à `get_dirspace_stats` Python ; `GET ?path=` conservé en confort (même réponse) |
| **Metadata** (`database_endpoint.py`) | | | |
| `GET /api/metadata/torrents/{ih}/health` | idem | ✅ | `refresh=1` déclenche un scrape immédiat via le torrent checker |
| `GET /api/metadata/torrents/popular` | idem | ✅ | Tri par seeders desc depuis `channel_node`+`torrent_state` |
| `GET /api/metadata/torrents/health` | idem | ✅ | Historique de santé |
| `GET /api/metadata/search/local?fts_text=` | idem | ✅ | LIKE sur `channel_node.title` (pas de FTS5 — résultats identiques, ordre simple) |
| `GET /api/metadata/search/completions` | idem | ✅ | Titres par préfixe |
| `GET /api/metadata/search/vocabulary` | idem | ✅ | `{"vocabularies": []}` (pas de FTS) |
| `PUT`/`DELETE`/`PATCH /api/metadata/torrents/{ih}/tags` | idem | ✅ | Tags en colonne `tags` (CSV) de `channel_node` |
| **Recherche distante** (`search_endpoint.py`) | | | |
| `PUT /api/search/remote?fts_text=` | idem | ✅ | `RemoteSelect` vers les pairs de la community content-discovery ; réponses intégrées à `channel_node`. 400 si IPv8 inactif |
| **Torrentinfo / createtorrent** | | | |
| `POST /api/torrentinfo/uri` | idem | ✅ | `file://`, `http(s)://` (fetch anti-SSRF), `magnet:` (résolution DHT ; `skipmagnet` = réponse immédiate) |
| `PUT /api/torrentinfo/file` | idem | ✅ | Corps brut bencode ou `{"torrent": "<hex>"}` |
| `POST /api/createtorrent` | idem | ✅ | Délégué à `librqbit::create_torrent` ; `export_dir`/`name`/`tracker`/`piece_length` |
| `POST /api/createtorrent/dryrun` | idem | ✅ | Vérifie l'écriture dans `export_dir` |
| **Session moteur** (`libtorrent_endpoint.py`) | | | |
| `GET /api/libtorrent/settings?session=` | idem | ✅ | Sous-ensemble `EngineConfig` avec noms `lt::settings_pack` équivalents |
| `GET /api/libtorrent/session?session=` | idem | ✅ | `hop` 0 = moteur principal, 1..=3 = lanes anonymes |
| **IPv8** (`ipv8_endpoint.py`) | | | |
| `GET /api/ipv8/overlays` | idem | ✅ | Discovery + content-discovery + tunnel (pairs par community) |
| `GET /api/ipv8/tunnel/settings` | idem | ✅ | `peer_flags`, `circuits`, `community_id` |
| `GET /api/ipv8/tunnel/circuits` | idem | ✅ | Circuits connus (`circuits_info`) |
| `GET /api/ipv8/tunnel/relays` | idem | ✅ | Relais actifs |
| `GET /api/ipv8/tunnel/exits` | idem | ✅ | Sockets de sortie |
| `GET /api/ipv8/tunnel/swarms` | idem | ✅ | Swarms hidden services |
| `GET /api/ipv8/tunnel/peers` | idem | ✅ | Pairs tunnel + flags |
| **Fichiers** (`file_endpoint.py`) | | | |
| `GET /api/files/browse?path=&files=` | idem | ✅ | `..` en tête, dossiers d'abord ; `/` liste les lecteurs sous Windows |
| `GET /api/files/list?path=&recursively=` | idem | ✅ | Listing récursif par défaut |
| `GET /api/files/create?path=` | idem | ✅ | `create_dir_all` |
| **RSS** (`rss_endpoint.py`) | | | |
| `PUT /api/rss` | idem | ✅ | Remplace la liste des flux (`{"urls": [...]}`) |
| **Versioning** (`versioning_endpoint.py`) | | | |
| `GET /api/versioning/versions` | idem | ✅ | Sous-répertoires `v*` de `state_dir` |
| `GET /api/versioning/versions/current` | idem | ✅ | Version du crate |
| `GET /api/versioning/versions/check` | idem | ✅ | `has_version: false` — pas de trafic implicite |
| `DELETE /api/versioning/versions/{v}` | idem | ✅ | Supprime un sous-répertoire `v*` (jamais le courant ; nom validé) |
| `POST /api/versioning/upgrade` | idem | ✅ | `{"started": false}` — pas de migrateurs |
| `GET /api/versioning/upgrade/available` | idem | ✅ | `{"can_upgrade": false}` |
| `GET /api/versioning/upgrade/working` | idem | ✅ | `{"working": false}` |
| **Logging** (`logging_endpoint.py`) | | | |
| `GET /api/logging?max_lines=` | idem | ✅ | Queue du dernier fichier `state_dir/logs/*.log` (vide s'il n'y en a pas) |

## DTO `downloads[]` (miroir du dict `info` Python)

| Champ Python | Champ Rust | Écart connu |
| :--- | :--- | :--- |
| `name`, `progress`, `infohash`, `size`, `speed_down`, `speed_up` | identiques | — |
| `status` / `status_code` | identiques | codes `DownloadStatus` Python conservés (0..11) ; mapping : `Initializing`→`METADATA`/`WAITING_FOR_HASHCHECK`, `Checking`→`HASHCHECKING`, `Downloading`→`DOWNLOADING`, `Seeding`→`SEEDING`, `Paused/Stopped`→`STOPPED`, `Error`→`STOPPED_ON_ERROR` |
| `eta` | `eta` | float de secondes, formule `get_eta()` Python : `(1-progress)*size/max(download_rate,1e-6)` (0.0 sans taille connue) |
| `num_peers` | `max(peers_live, leechers scrapés)` | `num_peers` Python = `max(lt, scraped)` ; librqbit n'expose pas le compteur swarm lt → scrape `torrent_state` comme borne |
| `num_connected_peers` | `peers_live` (librqbit `AggregatePeerStats`) | pairs connectés, fidèle au sens Python |
| `num_seeds` | `torrent_state.seeders` (scrape) | Python = `max(lt, scraped)` ; sans compteur lt on retient le scrape du torrent checker |
| `num_connected_seeds` | 0 | librqbit ne distingue pas seeds/leechers connectés — divergence documentée |
| `all_time_upload/download/ratio` | cumuls librqbit | ratio = upload/download (0 si download=0) |
| `hops`, `anon_download` | remplis | `hops` = sauts de la lane anonyme du téléchargement (`anon_hops_map` de la session) ; `anon_download` = `hops > 0` — testé |
| `trackers`, `destination`, `error`, `streamable` | remplis | trackers union `announce`/`announce-list` + ajouts à chaud ; `destination` = `output_folder` effectif |
| `safe_seeding`, `upload_limit`, `download_limit`, `seeding_ratio`, `completed_dir`, `time_added`, `time_finished`, `queue_position`, `auto_managed`, `user_stopped` | remplis depuis la persistance | Réglages par download checkpointés en base (équivalent `DownloadConfig`/`dlcheckpoints`) ; `seeding_ratio` = individuel ou défaut `download_defaults` ; politique d'arrêt de seed appliquée par la boucle de progression (`seeding_mode`/`seeding_time`) |
| `total_pieces` | rempli | `info.lengths().total_pieces()` rqbit (métadonnées requises) |

## Topics d'événements SSE

| Topic Python | Émis par Rust | Notes |
| :--- | :--- | :--- |
| `events_start` | ✅ | message initial à la connexion (`public_key`, `version`, `sessions`) |
| `tribler_shutdown_started` | ✅ | `Notification::SessionStopping` |
| `download_state_changed` | ✅ | `Notification::DownloadProgress` + `DownloadStateChanged` |
| `torrent_finished` | ✅ | `Notification::DownloadFinished` |
| `new_torrent_metadata_created` | ✅ | `Notification::TorrentMetadataCreated` |

## Endpoints Python non couverts (écart assumé ou en suspens)

| Endpoint Python | Statut | Raison |
| :--- | :--- | :--- |
| `/api/webui` | ⛔ | UI — hors périmètre backend |
| `/api/knowledge` (GraphDB/Rules) | ⛔ | service « knowledge » non réimplémenté (V1 hors scope) |
| `/api/trustview`, `bandwidth` | ⛔ | community TrustChain absente de la V1 |
| `/api/downloads/{ih}/peerdna` | ⏳ | analyse d'identité de pair — non implémenté |
| `channels` (CRUD channels) | ⏳ | la base `channel_node` est prête ; l'édition de channels est une fonctionnalité ultérieure |
| `GET /api/rss` (listing des items) | ⏳ | le `RssManager` ne conserve pas les items en base — à faire si l'UI en a besoin |
