# API Web Tribler — Inventaire complet des fonctions

> Source de vérité : `D:\Projet\Tribler_sources\tribler` (Tribler 8.x)
> + sous-module `pyipv8/`. Consommateur : `src/tribler/ui/` (React/TS,
> `services/tribler.service.ts` + `services/ipv8.service.ts`).
>
> Ce document liste **toutes** les fonctions exposées au frontend web,
> leur emplacement d'implantation Python, leur pendant Rust
> (`crates/tribler-api`), les paramètres avec valeurs par défaut et
> bornes min/max quand elles existent.

## 1. Architecture du plan de contrôle HTTP

```text
aiohttp Application (RootEndpoint, tribler/core/restapi/rest_endpoint.py)
└── middlewares (ordre d'enregistrement dans RESTManager.__init__) :
      ui_middleware          → redirige tout chemin hors /api,/docs,/static,/ui vers /ui<path>
      ApiKeyMiddleware       → exige la clé API sauf pour /docs,/static,/ui
      error_middleware       → formate les exceptions en {"error":{handled,message}}
      required_components_middleware → 404 tant que les composants requis ne sont pas prêts
└── sous-applications (RESTManager.add_endpoint, session.py:160-171)
      /api/events        EventsEndpoint          restapi/events_endpoint.py
      /api/files         FileEndpoint            restapi/file_endpoint.py
      /api/ipv8          IPv8RootEndpoint        restapi/ipv8_endpoint.py → pyipv8/ipv8/REST/*
      /api/logging       LoggingEndpoint         restapi/logging_endpoint.py
      /api/settings      SettingsEndpoint        restapi/settings_endpoint.py
      /api/shutdown      ShutdownEndpoint        restapi/shutdown_endpoint.py
      /api/statistics    StatisticsEndpoint      restapi/statistics_endpoint.py
      /api/downloads     DownloadsEndpoint       core/libtorrent/restapi/downloads_endpoint.py
      /api/createtorrent CreateTorrentEndpoint   core/libtorrent/restapi/create_torrent_endpoint.py
      /api/libtorrent    LibTorrentEndpoint      core/libtorrent/restapi/libtorrent_endpoint.py
      /api/torrentinfo   TorrentInfoEndpoint     core/libtorrent/restapi/torrentinfo_endpoint.py
      /api/metadata      DatabaseEndpoint        core/database/restapi/database_endpoint.py
      /api/search        SearchEndpoint          core/content_discovery/restapi/search_endpoint.py
      /api/rss           RSSEndpoint             core/rss/restapi/endpoint.py
      /api/versioning    VersioningEndpoint      core/versioning/restapi/versioning_endpoint.py
      /ui                WebUIEndpoint           restapi/webui_endpoint.py (hors /api)
      /docs              Swagger UI (aiohttp_apispec, /docs/swagger.json)
```

### Conventions transverses

| Aspect | Valeur | Source |
| :--- | :--- | :--- |
| Authentification | header `X-Api-Key`, **ou** query `?key=`, **ou** cookie `api_key` | `rest_manager.py` `ApiKeyMiddleware` |
| Chemins exemptés d'auth | `/docs`, `/static`, `/ui` | idem |
| Corps max requête | **16 Mio** (`MAX_REQUEST_SIZE = 16 * 1024**2`) | `rest_endpoint.py:20` |
| Format d'erreur | `{"error": {"handled": bool, "message": str}}` | `rest_endpoint.py` `RESTResponse` |
| Codes HTTP utilisés | 200, 206 (stream), 400, 401, 404, 412, 413, 416, 500 | divers |
| Événements temps réel | **SSE** : `event: <topic>\ndata: <json>\n\n` | `events_endpoint.py` |
| Bind par défaut | `api/http_host = 127.0.0.1`, `api/http_port = 0` (aléatoire, publié dans `api/http_port_running`) | `tribler_config.py` |
| HTTPS | désactivé par défaut (`api/https_enabled = false`, `https_certfile`) | idem |

Légende statut Rust : ✅ implémenté · ⚠️ partiel/écart · ❌ absent · ⛔ hors périmètre.

---

## 2. `/api/downloads` — `DownloadsEndpoint`

Python : `core/libtorrent/restapi/downloads_endpoint.py` · Rust : `handlers/downloads.rs` + `downloads_extra.rs`

| Méthode | Route | Description | Paramètres (défaut ; min-max) | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/api/downloads` | Liste tous les téléchargements (actifs et inactifs) + checkpoints + `clierrors` | `get_peers` bool `"0"` ; `get_pieces` bool `"0"` ; `get_availability` bool `"0"` ; `infohash` str (filtre, ajoute peers/pieces/availability à ce seul download) ; `excluded` str (exclut un infohash) | ✅ (flags peers/pieces/availability acceptés mais ignorés) |
| PUT | `/api/downloads` | Démarre un téléchargement depuis un URI (magnet/http/file) **ou** un `.torrent` brut (`Content-Type: applications/x-bittorrent`, paramètres en query) | `uri*` str ; `anon_hops` int (≥0, >0 exige `safe_seeding`) ; `safe_seeding` bool ; `destination` str ; `completed_dir` str ; `selected_files` int[] ; `auto_managed` bool ; `only_metadata` bool `"false"` ; `cli` bool false | ✅ |
| GET | `/api/downloads/clierrors` | Vide et retourne la file d'erreurs non remontées en mode CLI (max 100 conservées) | — | ❌ |
| DELETE | `/api/downloads/{infohash}` | Supprime un téléchargement | `remove_data*` bool (corps JSON, obligatoire) | ✅ |
| PATCH | `/api/downloads/{infohash}` | Modifie un téléchargement | `state` : `resume`/`stop`/`recheck`/`move_storage` (+`dest_dir`*, +`completed_dir`) ; `selected_files` int[] (0..max_index) ; `file_priority` [index, prio] (**prio 0..7**) ; `anon_hops` int (**doit être seul**) ; `upload_limit` int o/s ; `download_limit` int o/s ; `seeding_ratio` float ; `seeding_ratio_default` bool ; `queue_position` : `queue_up`/`queue_top`/`queue_down`/`queue_bottom` ; `auto_managed` bool | ⚠️ (`state` resume/stop + `anon_hops` seul (`update_hops`, rollback si échec) ; `recheck`/`move_storage`/`selected_files`/`file_priority`/limites/`queue_position`/`auto_managed` → non implémentés) |
| GET | `/api/downloads/{infohash}/torrent` | Retourne le `.torrent` bencodé (`application/x-bittorrent`) | — | ✅ |
| PUT | `/api/downloads/{infohash}/trackers` | Ajoute un tracker + réannonce immédiate | `url*` str (corps JSON) | ✅ (sans réannonce à chaud — rqbit) |
| PUT | `/api/downloads/{infohash}/default_trackers` | Planifie l'ajout des trackers par défaut (`trackers_file`) au prochain handle | — | ❌ |
| DELETE | `/api/downloads/{infohash}/trackers` | Retire un tracker | `url*` str | ❌ |
| PUT | `/api/downloads/{infohash}/tracker_force_announce` | Force une annonce immédiate sur un tracker | `url*` str | ❌ |
| GET | `/api/downloads/{infohash}/files` | Fichiers du téléchargement (index, name posix, size, included, priority, progress) | — | ✅ |
| GET | `/api/downloads/{infohash}/stream/{fileindex}` | Stream HTTP du fichier (supporte `Range`, 206/416, `Accept-Ranges: bytes`, priorise les pièces) | `fileindex` : 0..max_index | ✅ |

Champs `downloads[]` retournés : `name, progress (0..1), infohash, speed_down, speed_up (o/s),
status, status_code (DownloadStatus 0..11), size (o), eta (s), num_peers, num_seeds,
num_connected_peers, num_connected_seeds, all_time_upload, all_time_download, all_time_ratio,
last_download, last_upload, trackers[], hops, anon_download, safe_seeding, upload_limit,
download_limit, seeding_ratio, destination, completed_dir, total_pieces, error, time_added,
time_finished, queue_position, auto_managed, user_stopped, streamable` (+ `peers`, `pieces`
base64, `availability` sur demande).

---

## 3. `/api/torrentinfo` — `TorrentInfoEndpoint`

Python : `core/libtorrent/restapi/torrentinfo_endpoint.py` · Rust : `handlers/torrentinfo.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| POST | `/api/torrentinfo/uri` | Résout les métadonnées d'un URI `file:`/`http(s):`/`magnet:` (unshorten → fetch → metainfo via DHT/swarm, timeout **60 s**). Peut passer par un circuit à `hops` sauts. Ajoute les métadonnées à `metadata.db` | corps JSON : `uri*` str ; `hops` int ≥0 ; `skipmagnet` bool false (réponse immédiate sans résoudre le magnet) | ✅ |
| PUT | `/api/torrentinfo/file` | Métadonnées d'un `.torrent` brut envoyé dans le corps | corps binaire bencode | ✅ |

Réponse : `{files: [{index,name,size}], name, description, download_exists, valid_certificate}`
(+ `infohash` pour `/file`).

---

## 4. `/api/createtorrent` — `CreateTorrentEndpoint`

Python : `core/libtorrent/restapi/create_torrent_endpoint.py` · Rust : `handlers/createtorrent.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| POST | `/api/createtorrent` | Crée un `.torrent` depuis des fichiers locaux, démarre le seeding et retourne le torrent en **base64** | `files*` str[] ; `filenames` str[] ; `name` str ; `description` str ; `trackers` str[] ; `export_dir` str (défaut `download_defaults/saveas` = `~/Downloads`) ; `initial_nodes` str[] (`"<host> <port>"`) ; `torrent_version` str `"v1"` ; query `download` bool | ✅ |
| POST | `/api/createtorrent/dryrun` | Vérifie qu'un torrent `name` peut être écrit dans `export_dir` (sonde d'écriture du 1er parent existant) | `name` str ; `export_dir` str | ✅ |

---

## 5. `/api/libtorrent` — `LibTorrentEndpoint`

Python : `core/libtorrent/restapi/libtorrent_endpoint.py` · Rust : `handlers/libtorrent.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/api/libtorrent/settings` | Réglages `lt::settings_pack` de la session pour `hop` (0 = session claire, 1..3 = sessions anonymisées) | `hop` int `"0"` | ✅ (paramètre `hop`, alias `session` supporté) |
| GET | `/api/libtorrent/session` | Compteurs de stats de la session (`session_stats` : `net.recv_bytes`, `dht.*`, …) | `hop` int `"0"` | ✅ (paramètre `hop`, alias `session` supporté) |

---

## 6. `/api/metadata` — `DatabaseEndpoint`

Python : `core/database/restapi/database_endpoint.py` · Rust : `handlers/metadata.rs`

Paramètres de requête communs (`sanitize_parameters` + `MetadataParameters`) :

| Paramètre | Type | Défaut | Bornes / notes |
| :--- | :--- | :--- | :--- |
| `first` | int | `1` | borne basse de fenêtrage (rowid inclus) |
| `last` | int | `50` | borne haute |
| `sort_by` | str | — | colonnes : `category, name, size, infohash, date, created, status, votes, subscribed, health` |
| `sort_desc` | bool | `true` | |
| `hide_xxx` | bool | `false` | filtre XXX |
| `category` | str | — | |
| `tags` | str[] | — | répétable |
| `metadata_type` | str[] | — | ex. `torrent`(300), `channel`(220) |
| `exclude_deleted` | bool | `false` | |
| `max_rowid` | int | — | pagination incrémentale |
| `channel_pk` | hex | — | exige `origin_id` |
| `origin_id` | int | — | |
| `popular` | bool | `false` | force `sort_by=HEALTH` |
| `include_total` | bool | `false` | ajoute `total`+`max_rowid` (coûteux) |
| `filter` | str | — | ajouté au texte FTS |
| `fts_text` | str | — | requis pour les recherches |

Routes :

| Méthode | Route | Description | Paramètres spécifiques | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/api/metadata/torrents/{infohash}/health` | Déclenche un check de santé (scrape trackers/DHT) — réponse immédiate `{"checking": true}`, résultat via events | `timeout` int **défaut 20 s** | ✅ |
| GET | `/api/metadata/torrents/health` | Historique de santé `{local: [...], remote: [...]}` | — | ✅ |
| GET | `/api/metadata/torrents/popular` | Torrents les plus populaires (`metadata_type=torrent`, tri santé) | params communs | ✅ |
| GET | `/api/metadata/search/local` | Recherche FTS locale dans `metadata.db` | `fts_text*` requis + params communs | ✅ (LIKE, pas FTS5) |
| GET | `/api/metadata/search/completions` | Auto-complétion d'une requête | `q*` str ; **max 5 termes** | ✅ |
| GET | `/api/metadata/search/vocabulary` | Vocabulaire de l'auto-correcteur (`AugmentedSearch`) | — | ✅ (vide sans FTS) |
| PUT | `/api/metadata/torrents/{infohash}/tags` | Ajoute un tag au torrent (CSV dans la colonne `tags`) | `tag*` str (query) | ✅ |
| DELETE | `/api/metadata/torrents/{infohash}/tags` | Retire un tag | `tag*` str | ✅ |
| PATCH | `/api/metadata/torrents/{infohash}/tags` | Remplace tous les tags | `tags*` str (CSV) | ✅ |

---

## 7. `/api/search` — `SearchEndpoint`

Python : `core/content_discovery/restapi/search_endpoint.py` · Rust : `handlers/search.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| PUT | `/api/search/remote` | Diffuse une requête de recherche aux pairs de `ContentDiscoveryCommunity`. **Réponses poussées via `/api/events`** (topic `remote_query_results`) | query : `fts_text*` + params communs `/api/metadata` + `uuid`, `channel_pk`, `origin_id`. Réponse : `{request_uuid, peers: [mid hex]}` | ✅ |

---

## 8. `/api/events` — `EventsEndpoint` (SSE)

Python : `restapi/events_endpoint.py` · Rust : `handlers/events.rs`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/api/events` | Flux SSE permanent `text/event-stream`. Message initial `events_start` `{public_key, version, sessions}` ; les erreurs survenues sans client sont rejouées à la 1re connexion | ✅ |
| GET | `/api/events/info` | Infos de l'endpoint : `{public_key, version, sessions}` | ❌ |

Topics relayés au GUI (`topics_to_send_to_gui`, notifier.py) :

`events_start` `{public_key,version,sessions}` · `torrent_status_changed` `{infohash,status}` ·
`tunnel_removed` `{circuit_id,circuit_class,bytes_up,bytes_down,uptime,additional_info}` ·
`tribler_new_version` `{version}` · `tribler_exception` `{error,traceback}` ·
`torrent_finished` `{infohash,name,hidden}` · `torrent_health_updated` `{infohash,num_seeders,num_leechers,last_check}` ·
`tribler_shutdown_state` `{state}` · `remote_query_results` `{query,results,uuid,peer}` ·
`low_space` `{disk_usage_data}` · `report_config_error` `{error}` · `ask_add_download` `{uri}` ·
`local_query_results` `{query,results}` (notifié, hors liste GUI mais émis par local_search).

---

## 9. `/api/settings` — `SettingsEndpoint`

Python : `restapi/settings_endpoint.py` · Rust : `handlers/settings.rs`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/api/settings` | Retourne **tout** l'arbre de configuration (`TriblerConfig` + ports runtime) | ✅ |
| POST | `/api/settings` | Merge récursif des clés JSON envoyées, écrit `configuration.json`, applique les limites de session si section `libtorrent` | ✅ |

### Arbre de configuration complet — valeurs par défaut et bornes

Source : `src/tribler/tribler_config.py` `DEFAULT_CONFIG` + `pyipv8/ipv8/configuration.py`.

#### `api/`

| Clé | Défaut | Bornes / notes |
| :--- | :--- | :--- |
| `key` | (générée à l'installation) | clé API hex |
| `http_enabled` | `true` | |
| `http_port` | `0` | 0 = port aléatoire, sinon 1024..65535 |
| `http_host` | `"127.0.0.1"` | |
| `https_enabled` | `false` | |
| `https_host` | `"127.0.0.1"` | |
| `https_port` | `0` | |
| `https_certfile` | `"https_certfile"` | chemin PEM |
| `http_port_running` / `https_port_running` | `0` | runtime, écrits par le core |

#### Racine

| Clé | Défaut | Notes |
| :--- | :--- | :--- |
| `headless` | `false` | mode sans GUI |
| `start_minimized` | `false` | |
| `statistics` | `false` | active les stats IPv8 |
| `state_dir` | `%APPDATA%/.Tribler` | |
| `memory_db` | `false` | DB en RAM |
| `tray_icon_color` | `""` | |
| `ui` | `{}` | réglages UI libres (sparse) |

#### `ipv8/` (hérité de pyipv8 `configuration.py`)

| Clé | Défaut | Bornes / notes |
| :--- | :--- | :--- |
| `interfaces` | `[{UDPIPv4, 0.0.0.0, 8090}, {UDPIPv6, ::, 8091}]` | `worker_threads` optionnel |
| `keys` | `[{anonymous id, curve25519, ec_multichain.pem}, {secondary, curve25519, secondary_key.pem}]` | |
| `logger.level` | `"INFO"` | DEBUG/INFO/WARNING/ERROR |
| `walker_interval` | `0.5` s | intervalle des stratégies de découverte |
| `overlays` | `[DiscoveryCommunity]` (filtré par Tribler) | walkers `RandomWalk`(20 pairs, timeout 3 s), `RandomChurn`(sample 8, ping 10 s, inactive 27.5 s, drop 57.5 s), `PeriodicSimilarity` ; bootstrapper `DispersyBootstrapper` (12 IP + 4 DNS, `bootstrap_timeout` 30 s) |

#### `libtorrent/`

| Clé | Défaut | Bornes / notes |
| :--- | :--- | :--- |
| `socks_listen_ports` | `[0,0,0,0,0]` | ports SOCKS5 (par lane d'anonymat) |
| `listen_interface` | `"0.0.0.0"` | |
| `port` | `0` | 0 = aléatoire |
| `listen_interface_v6` | `""` | |
| `port_v6` | `0` | |
| `proxy_type` | `0` | enum libtorrent : 0 aucun, 1 socks4, 2 socks5, 3 socks5+auth, 4 http, 5 http+auth, 6 i2p |
| `proxy_server` / `proxy_auth` | `""` | `host:port` / `user:pass` |
| `max_connections_download` | `-1` | -1 = illimité |
| `max_download_rate` / `max_upload_rate` | `0` | 0 = illimité (o/s) |
| `use_advanced_rate_limits` | `false` | |
| `advanced_rate_limits` | `{str(i-1): "[[null,null]×48]"}` i=0..3 | limites horaires par hop (JSON packé) |
| `utp` | `true` | |
| `dht` | `true` | |
| `dht_readiness_timeout` | `30` s | |
| `upnp` / `natpmp` / `lsd` | `true` | |
| `announce_to_all_tiers` / `announce_to_all_trackers` | `false` | |
| `max_concurrent_http_announces` | `50` | |
| `check_after_complete` | `false` | re-hash après complétion |
| `active_downloads` | `3` | file d'attente libtorrent |
| `active_seeds` | `5` | |
| `active_checking` | `1` | |
| `active_dht_limit` | `88` | |
| `active_tracker_limit` | `1600` | |
| `active_lsd_limit` | `60` | |
| `active_limit` | `500` | |
| `ask_download_settings` | `false` | si true + `cli` → event `ask_add_download` |
| `clear_orphaned_parts` | `false` | |
| `allow_mmap` | `true` | fichiers mappés mémoire |

#### `libtorrent/download_defaults/`

| Clé | Défaut | Bornes / notes |
| :--- | :--- | :--- |
| `anonymity_enabled` | `true` | |
| `number_hops` | `1` | 0 = non anonyme ; **max 3** sauts (validation speedtest `1..3`) |
| `safeseeding_enabled` | `true` | requis si `anon_hops>0` |
| `saveas` | `~/Downloads` | |
| `seeding_mode` | `"forever"` | enum : `forever` / `never` / `ratio` / `time` |
| `seeding_ratio` | `2.0` | seuil si mode `ratio` |
| `seeding_time` | `60.0` | seuil si mode `time` (s) |
| `channel_download` | `false` | |
| `add_download_to_channel` | `false` | |
| `trackers_file` | `""` | liste de trackers par défaut |
| `trackers_file_sync_url` | `""` | |
| `torrent_folder` | `""` | backup des .torrent |
| `auto_managed` | `false` | |
| `completed_dir` | `""` | déplacement après complétion |

Défauts par download (`DownloadConfig`, `download_config.py`) : `hops=0`, `safe_seeding=false`,
`user_stopped=false`, `share_mode=false`, `upload_mode=false`, `upload_limit=-1`,
`download_limit=-1`, `auto_managed=false`, `stop_after_metainfo=false`.

#### Autres sections

| Clé | Défaut | Notes |
| :--- | :--- | :--- |
| `content_discovery_community/enabled` | `true` | |
| `database/enabled` | `true` | |
| `dht_discovery/enabled` | `true` | |
| `recommender/enabled` | `true` | ⚠️ aucun endpoint serveur (voir §13) |
| `rendezvous/enabled` | `true` | |
| `rss/enabled` | `true` ; `rss/urls` `[]` | |
| `torrent_checker/enabled` | `true` | |
| `tunnel_community/enabled` | `true` | |
| `tunnel_community/min_circuits` | `3` | injecté dans `TunnelSettings.min_circuits` |
| `tunnel_community/max_circuits` | `8` | → `TunnelSettings.max_circuits` |
| `versioning/enabled` | `true` ; `allow_pre` `false` | |
| `watch_folder/enabled` | `false` ; `directory` `""` ; `check_interval` `10.0` s | |

#### `TunnelSettings` pyipv8 (exposé via `/api/ipv8/tunnel/settings`)

Source : `pyipv8/messaging/anonymization/community.py` + `core/tunnel/community.py`.

| Clé | Défaut | Notes |
| :--- | :--- | :--- |
| `min_circuits` | `1` (Tribler : 3) | |
| `max_circuits` | `8` | |
| `max_joined_circuits` | `100` | relais max acceptés |
| `max_time` | `3600` s | durée de vie max d'un circuit |
| `max_time_ip` | `86400` s | durée de vie d'un introduction point |
| `max_time_inactive` | `20` s | |
| `max_traffic` | `10 Gio` | par circuit (config pyipv8 : 250 Mio) |
| `circuit_timeout` | `60` s | |
| `unstable_timeout` | `60` s | |
| `next_hop_timeout` | `10` s | |
| `swarm_lookup_interval` | `30` s | |
| `swarm_connection_limit` | `15` | |
| `remove_tunnel_delay` | `5` s | |
| `peer_flags` | `{RELAY, SPEED_TEST}` (+ `EXIT_BT/IPV8/HTTP` si `exitnode_enabled`) | |
| `max_relay_early` | `8` | |
| `exitnode_enabled` | `false` | Tribler |
| `default_hops` | `0` | Tribler |
| `max_intro_points` | `10` | Tribler |

---

## 10. `/api/statistics` — `StatisticsEndpoint`

Python : `restapi/statistics_endpoint.py` · Rust : `handlers/statistics.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/api/statistics/tribler` | `{peers, db_size, num_torrents, socks5_sessions[{hops,sessions,associates}], libtorrent{sessions[],total_recv_bytes,total_sent_bytes}, endpoint_version}` | — | ✅ |
| GET | `/api/statistics/ipv8` | `{ipv8_statistics: {total_up, total_down}}` | — | ✅ |
| PUT | `/api/statistics/dirspace` | Espace disque `{total,used,free}` du dossier (remonte au 1er parent existant ; défaut `download_defaults/saveas`) | corps JSON `directory` str | ⚠️ GET `?path=` côté Rust |

---

## 11. `/api/files`, `/api/logging`, `/api/rss`, `/api/versioning`, `/api/shutdown`

### `FileEndpoint` — `restapi/file_endpoint.py` → `handlers/files.rs`

| Méthode | Route | Description | Paramètres (défaut) | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/api/files/browse` | Navigation FS : liste dossiers (+fichiers si demandé), `..` en tête ; `/` sous Windows = liste des lecteurs | `path` `""` ; `files` `"0"` (1 pour inclure les fichiers) | ✅ |
| GET | `/api/files/list` | Liste plate des fichiers d'un dossier | `path` `""` ; `recursively` `"1"` (0 pour non récursif) | ✅ |
| GET | `/api/files/create` | Crée un dossier | `path` ; `recursively` `"1"` | ✅ |

### `LoggingEndpoint` — `restapi/logging_endpoint.py` → `handlers/logging.rs`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/api/logging` | Texte des **400** derniers records de log (buffer circulaire `RotatingMemoryHandler(400)`) | ✅ (`?max_lines=` en plus) |

### `RSSEndpoint` — `core/rss/restapi/endpoint.py` → `handlers/rss.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| PUT | `/api/rss` | Remplace la liste des flux surveillés (`rss/urls` persisté) | corps `{urls*: str[]}` | ✅ |

### `VersioningEndpoint` — `core/versioning/restapi/versioning_endpoint.py` → `handlers/versioning.rs`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/api/versioning/versions` | Sous-versions présentes dans `state_dir` + `current` | ✅ |
| GET | `/api/versioning/versions/current` | Version courante (`"git"` si source) | ✅ |
| GET | `/api/versioning/versions/check` | Sonde tribler.org/GitHub → `{new_version, has_version}` | ⚠️ (`has_version:false` — pas de trafic réseau implicite) |
| DELETE | `/api/versioning/versions/{version}` | Supprime l'état d'une ancienne version | ✅ |
| POST | `/api/versioning/upgrade` | Lance la migration des données anciennes | ⚠️ stub `{success}` |
| GET | `/api/versioning/upgrade/available` | `{can_upgrade}` — anciennes données détectées | ✅ (toujours false) |
| GET | `/api/versioning/upgrade/working` | `{running}` — upgrade en cours | ✅ |

### `ShutdownEndpoint` — `restapi/shutdown_endpoint.py` → `handlers/shutdown.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| PUT | `/api/shutdown` | Arrête le daemon (event `shutdown_event`) | `restart` `"0"` (1 = redémarrage demandé) | ✅ |

---

## 12. `/api/ipv8/*` — sous-endpoints pyipv8

`IPv8RootEndpoint` (`restapi/ipv8_endpoint.py`) monte `pyipv8/ipv8/REST/root_endpoint.py`
qui enregistre 8 sous-endpoints. Rust : `handlers/ipv8.rs`.

### `/api/ipv8/asyncio` — `REST/asyncio_endpoint.py`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/drift` | Mesures de dérive du tick asyncio (`{measurements:[{timestamp,drift}]}`, historique 100) | — | ❌ |
| PUT | `/drift` | Active/désactive la mesure de dérive | `enable*` bool | ❌ |
| GET | `/tasks` | Toutes les tâches asyncio (name, running, stack, taskmanager, start_time, interval) | — | ❌ |
| PUT | `/debug` | Options debug asyncio | `enable` bool ; `slow_callback_duration` int s | ❌ |
| GET | `/debug` | Messages du log asyncio (deque 50) + état debug | — | ❌ |

### `/api/ipv8/dht` — `REST/dht_endpoint.py`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/dht/statistics` | Stats DHT : peer_id, node_id, routing_table_size/buckets, num_keys_in_store, num_tokens, store peers | — | ✅ |
| GET | `/dht/values` | Valeurs stockées localement (par interface) | — | ✅ |
| GET | `/dht/values/{key}` | Lookup de valeurs + debug crawl (`requests,responses,time`) | `key` hex | ✅ |
| PUT | `/dht/values/{key}` | Stocke une valeur signée | `value*` hex | ✅ |
| GET | `/dht/peers/{mid}` | connect_peer par mid (sha1 pubkey) | `mid` hex | ✅ |
| GET | `/dht/buckets` | Buckets de la routing table (prefix, peers, distance, failed, last_contact) | — | ✅ (`distance` en décimale exacte, chaîne) |
| GET | `/dht/buckets/{prefix}/refresh` | Rafraîchit un bucket | `prefix` `\w*` | ✅ |

### `/api/ipv8/identity` — `REST/identity_endpoint.py` (self-sovereign identity)

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/identity` | Liste les pseudonymes | ❌ |
| GET | `/identity/{p}/schemas` | Schémas d'attributs disponibles | ❌ |
| GET | `/identity/{p}/public_key` | Clé publique (b64) du pseudonyme | ❌ |
| GET | `/identity/{p}/unload` · `/remove` | Décharge / supprime un pseudonyme | ❌ |
| GET | `/identity/{p}/credentials` · `/credentials/{subject_key}` | Attributs de moi / d'un sujet | ❌ |
| GET | `/identity/{p}/peers` | Pairs du canal d'identité | ❌ |
| PUT | `/identity/{p}/allow/{verifier}` · `/disallow/{verifier}` | Autorise/interdit la vérification (`name*`) | ❌ |
| PUT | `/identity/{p}/request/{authority}` | Demande d'attestation (`name*`, `schema*`, `metadata`) | ❌ |
| PUT | `/identity/{p}/attest/{subject}` · `/verify/{subject}` | Atteste / vérifie un attribut | ❌ |
| GET | `/identity/{p}/outstanding/attestations` · `/verifications` | Demandes en cours | ❌ |
| GET | `/identity/{p}/verifications` | Sorties de vérification | ❌ |

### `/api/ipv8/isolation` — `REST/isolation_endpoint.py`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| POST | `/isolation` | Injecte une adresse : pair bootstrap (blacklisté du walk) ou exit node | `ip*` str ; `port*` int ; `bootstrapnode` bool ; `exitnode` bool (l'un des deux requis) | ✅ |

### `/api/ipv8/network` — `REST/network_endpoint.py`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/network` | Pairs vérifiés connus `{mid b64: {ip, port, public_key, services}}` | ✅ |

### `/api/ipv8/noblockdht` — `REST/noblock_dht_endpoint.py`

| Méthode | Route | Description | Rust |
| :--- | :--- | :--- | :--- |
| GET | `/noblockdht/{mid}` | `connect_peer` non bloquant (fire-and-forget) | ✅ |

### `/api/ipv8/overlays` — `REST/overlays_endpoint.py` → `handlers/ipv8.rs`

| Méthode | Route | Description | Paramètres | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/overlays` | Toutes les communautés chargées (id, my_peer, peers, strategies, max_peers, is_isolated, wan/lan estimés, stats) | — | ✅ |
| GET | `/overlays/statistics` | Stats réseau par overlay (`"id:handler"` → `NetworkStat.to_dict()`) | — | ✅ |
| POST | `/overlays/statistics` | Active/désactive les stats par overlay | `enable*` bool ; `all` bool ; `overlay_name` str | ✅ |

### `/api/ipv8/tunnel` — `REST/tunnel_endpoint.py` → `handlers/ipv8.rs`

| Méthode | Route | Description | Paramètres (défaut ; min-max) | Rust |
| :--- | :--- | :--- | :--- | :--- |
| GET | `/tunnel/settings` | Réglages `TunnelSettings` (cf. table §9) | — | ✅ |
| GET | `/tunnel/circuits` | Circuits : `circuit_id, goal_hops, actual_hops, verified_hops[ mids], unverified_hop, type, state, bytes_up/down, creation_time, exit_flags` | — | ✅ |
| GET | `/tunnel/circuits/test` | Speedtest sur un **nouveau** circuit (détruit après), flux SSE `speed: {"up","down"} MiB/s` | `goal_hops` `"1"` (**1..3**) ; `request_size` `50` (**0..2000**) ; `response_size` `1024` (**0..2000**) ; `test_time_ms` `"5000"` (**1..60000**) | ❌ |
| GET | `/tunnel/circuits/{circuit_id}/test` | Speedtest d'un circuit existant (doit être `READY` + flag `PEER_FLAG_SPEED_TEST`) | mêmes bornes, sans `goal_hops` | ❌ |
| GET | `/tunnel/relays` | Relais : `circuit_from, circuit_to, is_rendezvous, direction forward/backward, bytes_up/down, creation_time` | — | ✅ |
| GET | `/tunnel/exits` | Sockets de sortie : `circuit_from, enabled, bytes_up/down, creation_time, is_introduction, is_rendezvous` | — | ✅ |
| GET | `/tunnel/swarms` | Swarms hidden services : `info_hash, num_seeders, num_connections(_incomplete), num_ips_from_dht/pex, seeding, last_lookup, bytes_up/down` | — | ✅ |
| GET | `/tunnel/swarms/{infohash}/size` | Estimation de taille d'un swarm caché | `hops` `1` | ❌ |
| GET | `/tunnel/peers` | Pairs tunnel : `ip, port, mid, is_key_compatible, flags[]` | — | ✅ |
| GET | `/tunnel/peers/dht` | Introduction points du store DHT local | — | ❌ |
| GET | `/tunnel/peers/pex` | Introduction points du store PEX local | — | ❌ |

---

## 13. Hors `/api` et cas particuliers

| Route | Rôle | Rust |
| :--- | :--- | :--- |
| `GET /docs`, `/docs/swagger.json` | Swagger UI auto-généré (aiohttp_apispec) | ⛔ |
| `GET /ui/{path}` | Sert le frontend (`webui_root/dist`, proxy `localhost:5173` en dev) | ⛔ UI |
| `GET /{autre}` | `ui_middleware` redirige vers `/ui<path>` | ⛔ |
| `PUT /api/recommender/clicked` | **Appelé par l'UI** (`tribler.service.ts`) `{query, chosen_index, timestamp, results[]}` — **aucun handler Python** : retourne 404 | ⛔ absent même en Python |

### Routes Python non encore portées dans `tribler-api` (récapitulatif ❌)

- `GET /api/events/info`
- `GET /api/downloads/clierrors`
- `PUT /api/downloads/{ih}/default_trackers`, `DELETE /api/downloads/{ih}/trackers`,
  `PUT /api/downloads/{ih}/tracker_force_announce`
- IPv8 : tout `asyncio/*`, `identity/*`, `tunnel/circuits/*/test`,
  `tunnel/swarms/{ih}/size`, `tunnel/peers/dht`, `tunnel/peers/pex`
- Écarts de signature : `PUT /api/statistics/dirspace` (Rust = GET `?path=`),
  `?hop=` libtorrent (Rust = `?session=`).
- Écarts de type/valeur dans `GET /api/downloads` : `eta` est une chaîne
  formatée (`"3m 20s"`) alors que Python émet un `Integer`/`float` de
  secondes ; `num_seeds`/`num_connected_seeds` fixés à 0 (pas de scrape
  intégré au listing) ; `trackers` fixé à `[]` ; `safe_seeding` fixé à
  `false` (le drapeau n'est pas persisté par download — la condition
  `anon_hops>0 ⇒ safe_seeding` est en revanche appliquée au `PUT`).

---

*Document généré par analyse statique des sources `D:\Projet\Tribler_sources\tribler`
(rev. checkout local) et `crates/tribler-api`. Complément statut :
`docs/reference_tribler/api_rest_mapping.md`.*
