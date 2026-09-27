# Mapping API REST — Tribler Python ↔ `tribler-api` (Rust)

Références : `D:\Projet\Tribler_sources\tribler\src\tribler\core\restapi\`
(`rest_manager.py`, `events_endpoint.py`) et
`core\libtorrent\restapi\downloads_endpoint.py`.

Convention générale :

- Préfixe `/api` identique au Python.
- Erreurs au format `{"error": {"handled": bool, "message": str}}`
  (identique à `rest_manager.py`).
- Bind `127.0.0.1` uniquement (API de controle locale).

## Endpoints implémentés (étape 6)

| Python | Rust | Statut | Notes |
| :--- | :--- | :--- | :--- |
| `GET /api/downloads` | `GET /api/downloads` | ✅ | `get_peers`/`get_pieces`/`get_availability` acceptés mais ignorés ; `infohash` et `excluded` (filtres) implémentés. Réponse : `{"downloads": [...], "checkpoints": {...}, "clierrors": 0}` |
| `PUT /api/downloads` | `PUT /api/downloads` | ✅ | `uri` (magnet/http) et `torrent` (chemin local) supportés ; `destination`, `paused` acceptés (`destination` ignoré à ce stade) ; `anon_hops > 0` → 400 (tunnels = étape 12) |
| `DELETE /api/downloads/{infohash}` | `DELETE /api/downloads/{infohash}` | ✅ | `remove_data` supporté |
| `PATCH /api/downloads/{infohash}` | `PATCH /api/downloads/{infohash}` | ✅ | `state=resume`/`stop` implémentés ; `recheck`, `move_storage` → 400 ; `anon_hops` → 400 |
| `GET /api/events` | `GET /api/events` | ✅ | **SSE** (pas WebSocket — comme le Python) : trames `event: <topic>\ndata: <json>\n\n`, message initial `events_start` |

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

## Endpoints Python non encore couverts

Prévus pour les étapes ultérieures : `/api/shutdown`, `/api/settings`,
`/api/statistics/*`, `/api/file`, `/api/ipv8/*`, `/api/webui`,
`/api/downloads/{ih}/files|trackers|peerdna`, endpoints `metadata`,
`search`, `channels`, `popular_torrents`, `rss`, `torrentinfo`,
`createtorrent`, `versioning`, `knowledge`… (couverture complète =
étape 15, cf. `docs/plans/roadmap.md`).
