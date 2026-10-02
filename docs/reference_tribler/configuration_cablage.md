# Câblage `configuration.json` → comportement — correspondance et écarts

État après les lots de câblage (commits `fd76e81`, `edb7d9c`,
`13b9e8b`, `26c870a`) : tous les champs de `DaemonConfig` sont lus.
Ce fichier recense les champs **sans équivalent implémentable** et les
écarts de comportement assumés — le détail des valeurs/bornes est dans
`api_endpoints_complet.md`.

## Champs câblés vers une décision explicite (pas d'équivalent moteur)

Ces clés sont lues au démarrage ; quand elles demandent un comportement
que `librqbit` (ou le portage actuel) n'offre pas, le daemon émet un
log (`warn`/`debug`) plutôt que de les ignorer silencieusement.

| Clé | Défaut | Décision |
| :--- | :--- | :--- |
| `libtorrent/natpmp` | `true` | librqbit ne supporte pas NAT-PMP — `warn` si activé ; UPnP (`upnp`) couvre le besoin. |
| `libtorrent/announce_to_all_tiers` | `false` | rqbit annonce à tous les trackers sans tiering — le flag ne peut rien changer, `debug`. |
| `libtorrent/announce_to_all_trackers` | `false` | idem — `debug`. |
| `libtorrent/max_concurrent_http_announces` | `50` | pas de limite d'annonces HTTP concurrentes exposée par rqbit — `debug`. |
| `libtorrent/active_dht_limit` | `88` | limite de débit par fonction (libtorrent) sans équivalent rqbit — `debug`. |
| `libtorrent/active_tracker_limit` | `1600` | idem — `debug`. |
| `libtorrent/active_lsd_limit` | `60` | idem — `debug`. |
| `ipv8/interfaces[].worker_threads` | `null` | inerte : Tokio gère le multi-threading, pas de worker par interface. |
| `ipv8/statistics` | `false` | inerte : clé morte dans Tribler 8.x même (résidu pyipv8 `StatisticsIPv8`). |
| `recommender` / `rendezvous` | `enabled=false` | clés mortes dans Tribler 8.x même (jamais relues après `tribler_config.py`) — fonctions historiques absorbées : `recommender` → tâche périodique de re-vérification du `torrent_checker` (`check_oldest`), `rendezvous` → points de rendez-vous des hidden services dans `TunnelCommunity`. `enabled` ignoré (parité Python) — `debug`. |

## Application à chaud (`POST /api/settings`)

Tout champ modifié est **persisté** immédiatement dans
`configuration.json` (merge récursif + `config.write()`, clés
inconnues préservées via `extra`). Appliqué **à chaud** (sans
redémarrage) — parité `set_session_limits` Python + services :

| Champ | Application à chaud |
| :--- | :--- |
| `rss/urls` | `RssManager::update` |
| `watch_folder/dir` | redémarrage du service |
| `libtorrent/download_defaults/saveas` | dossier des **nouveaux** ajouts (`effective_output_dir`) |
| `libtorrent/max_download_rate`/`max_upload_rate` | `Session::ratelimits` rqbit sur toutes les lanes |
| `libtorrent/active_downloads`/`active_seeds`/`active_limit` | relus par `enforce_queue_limits` au tick suivant |
| `destination` (`PUT /api/downloads`) | `AddDownloadOptions.output_folder`, persisté en `downloads.output_dir` |
| tout le reste | restart-only — comme Python (seul `set_session_limits` est à chaud chez Tribler) ; `GET /api/settings` reflète la valeur **postée** (en attente de redémarrage), pas l'état runtime — `ServiceOverrides` mémorise `ipv8`/`engine`/`torrent_checker` postés pour `effective_config()` |

## Écarts de comportement assumés

| Domaine | Python Tribler | Portage Rust |
| :--- | :--- | :--- |
| `api/https_certfile` absent/invalide | échec au `load_cert_chain` (le site HTTPS ne démarre pas) | certificat auto-signé `rcgen` (SAN `localhost`/`127.0.0.1`/`::1`) généré et écrit au chemin configuré |
| `libtorrent/allow_mmap` | backend libtorrent mmap | `MmapFilesystemStorageFactory` rqbit quand `true` |
| `libtorrent/clear_orphaned_parts` | purge des `.parts` orphelins libtorrent | purge des `*.parts` sans torrent associé dans `saveas` au démarrage de session |
| `libtorrent/check_after_complete` | `force_recheck` libtorrent | `session.recheck` rqbit sur transition vers `Seeding` |
| `libtorrent/active_*` (file) | gestionnaire interne libtorrent | queue manager `onionbit-core` (`enforce_queue_limits`) : seuls les torrents `auto_managed` comptent, pause/reprise par `queue_position` |
| `tray_icon_color` | recoloration de l'icône `.ico` | carré plein recoloré `#RRGGBB` (la ressource `.ico` n'est pas recolorable) |
| `start_minimized` | l'UI ne s'ouvre pas (même processus que le core) | inerte : le daemon ne lance jamais l'UI (c'est `onionbit_ui.exe` qui démarre le daemon) |

## Mapping direct (récapitulatif)

- `api/http_*` → listener HTTP axum (`http_port_running` réécrit).
- `api/https_*` → listener TLS `axum-server` (rustls), même routeur,
  `https_port_running` réécrit.
- `libtorrent/listen_interface`/`port`/`listen_interface_v6`/`port_v6`
  → `ListenerOptions` rqbit (dual-stack si IPv6 configurée).
- `libtorrent/utp`/`upnp`/`max_connections_download`/`proxy_*`/
  `max_*_rate` → `EngineConfig` → `SessionOptions`/`ListenerOptions`.
- `libtorrent/active_checking` → `SessionOptions.concurrent_init_limit`.
- `libtorrent/active_downloads`/`active_seeds`/`active_limit` →
  `QueueLimits` du gestionnaire de file.
- `libtorrent/dht_readiness_timeout` → attente table de routage DHT
  avant démarrage effectif de session.
- `libtorrent/socks_listen_ports` → ports SOCKS5 des lanes anonymes
  (`[hops-1]`, `0` = éphémère, repli éphémère + warn si occupé).
- `tunnel_community/min_circuits`/`max_circuits` → cible et plafond du
  watchdog de circuits (clamp à la `monitor_downloads`).
- `ipv8/interfaces[UDPIPv6]` → second socket `UdpEndpoint` IPv6.
- `ipv8/logger_level` → directive par-crate fusionnée dans l'`EnvFilter`.
- `content_discovery_community/enabled` → gate de
  `ContentDiscoveryCommunity`.
- `database/enabled=false` → `db_filename=":memory:"`.
- `versioning/enabled=false` → routes `/api/versioning/*` absentes (404).
- `versioning/github_repo` → dépôt `owner/repo` sondé par
  `versions/check` (défaut : champ `repository` du workspace, vide =
  pas de sonde GitHub) ; `versioning/check_urls` → sondes
  additionnelles (`{current}` substitué, équivalent
  `release.tribler.org`) ; `versioning/allow_pre` → sonde la liste
  GitHub `releases?per_page=1` (pré-versions) au lieu de
  `releases/latest` ; `versioning/check_timeout_secs` → timeout par
  sonde (défaut 5 = `ClientTimeout` Python).
- `headless` → pas de systray ; `start_minimized` → inerte côté daemon
  (le daemon ne lance jamais l'UI ; la clé reste lue/conservée pour la
  future UI, qui l'appliquera à sa propre fenêtre).
- `download_defaults/torrent_folder` → backup `<name> [<ih>].torrent`.
- `channel_download`/`add_download_to_channel` → colonnes DB v6,
  propagées par téléchargement.
