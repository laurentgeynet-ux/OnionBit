# Mapping API REST — Tribler Python ↔ `tribler-api` (Rust)

Références : `D:\Projet\Tribler_sources\tribler\src\tribler\core\restapi\`
(`rest_manager.py`, `events_endpoint.py`) et
`core\libtorrent\restapi\downloads_endpoint.py`.

Convention générale :

- Préfixe `/api` identique au Python.
- Erreurs au format `{"error": {"handled": bool, "message": str}}`
  (identique à `rest_manager.py`).
- Bind `127.0.0.1` uniquement (API de controle locale).

## Endpoints implémentés

| Python | Rust | Statut | Notes |
| :--- | :--- | :--- | :--- |
| **Downloads** (`downloads_endpoint.py`) | | | |
| `GET /api/downloads` | `GET /api/downloads` | ✅ | `get_peers`/`get_pieces`/`get_availability` acceptés mais ignorés ; `infohash` et `excluded` (filtres) implémentés. Réponse : `{"downloads": [...], "checkpoints": {...}, "clierrors": 0}` |
| `PUT /api/downloads` | `PUT /api/downloads` | ✅ | `uri` (magnet/http) et `torrent` (chemin local) ; `destination`, `paused` acceptés ; `anon_hops` routé vers la lane anonyme du stack IPv8 (étape 15) |
| `DELETE /api/downloads/{infohash}` | `DELETE /api/downloads/{infohash}` | ✅ | `remove_data` supporté |
| `PATCH /api/downloads/{infohash}` | `PATCH /api/downloads/{infohash}` | ✅ | `state=resume`/`stop` ; `recheck`, `move_storage` → 400 |
| `GET /api/downloads/{ih}/torrent` | `GET /api/downloads/{ih}/torrent` | ✅ | `.torrent` brut, `Content-Type: application/x-bittorrent` |
| `GET`/`PUT /api/downloads/{ih}/trackers` | idem | ✅ | `PUT {"url"}` ajoute un tracker à chaud (rqbit ne réannonce pas à chaud — effectif à la session suivante) |
| `GET /api/downloads/{ih}/files` | `GET /api/downloads/{ih}/files` | ✅ | index, name, size, progress par fichier |
| `GET /api/downloads/{ih}/stream/{fileindex}` | idem | ✅ | flux HTTP chunked avec `start` (seek) ; borné par le buffer rqbit |
| `GET /api/events` | `GET /api/events` | ✅ | **SSE** : trames `event: <topic>\ndata: <json>\n\n`, message initial `events_start` |
| **Settings / shutdown / stats** | | | |
| `GET /api/settings` | `GET /api/settings` | ✅ | Arbre `ipv8`/`tunnel_community`/`libtorrent`/`watch_folder`/`rss`/`torrent_checker`/`api` ; reflète les overrides à chaud |
| `POST /api/settings` | `POST /api/settings` | ✅ | Application à chaud : `rss.urls`, `watch_folder.enabled`/`directory` ; le reste accepté (redémarrage requis) |
| `PUT /api/shutdown` | `PUT /api/shutdown` | ✅ | Réponse immédiate, arrêt en tâche de fond |
| `GET /api/statistics/tribler` | idem | ✅ | `db_size`, `num_torrents`, `num_channels`, `peers`, `libtorrent.sessions` (lanes anonymes) |
| `GET /api/statistics/ipv8` | idem | ✅ | `total_up`/`total_down` de l'endpoint UDP |
| `GET /api/statistics/dirspace?path=` | `GET` | ✅ | `fs2` `total`/`free`/`used` |
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
| `eta` | `eta` | Rust émet une chaîne formatée (`"3m 20s"`) ou `""`, Python un float de secondes — à harmoniser à l'étape 15 si besoin |
| `num_peers`, `num_connected_peers` | `peers_live` (librqbit `AggregatePeerStats`) | librqbit ne distingue pas seeds/leechers connectés |
| `num_seeds`, `num_connected_seeds` | 0 | nécessite le scraping de trackers (étape 14, `torrent_checker`) |
| `all_time_upload/download/ratio` | cumuls librqbit | ratio = upload/download (0 si download=0) |
| `trackers`, `hops`, `anon_download`, `safe_seeding`, `upload_limit`, `download_limit`, `seeding_ratio`, `destination`, `completed_dir`, `total_pieces`, `error`, `time_added`, `time_finished`, `queue_position`, `auto_managed`, `user_stopped`, `streamable` | émis avec valeurs par défaut | champs présents pour compatibilité clients ; remplis au fil des étapes (tunnels=12, services=14) |

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
