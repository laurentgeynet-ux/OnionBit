# Revue des garde-fous réseau (Étape 16)

Inventaire des protections, leur emplacement unique et le test qui les
couvre. Règle absolue : **aucun repli silencieux** — une politique qui
échoue ferme la connexion, jamais de fallback direct.

## 1. Anti-SSRF — `tribler-network-policy/src/address_policy.rs`

| Contrôle | Emplacement | Test |
| :--- | :--- | :--- |
| Refus loopback/privé/link-local/multicast/non-routable/doc | `IpPolicy::strict()` → `check(addr)` | `tribler-core/tests/policy.rs::http_uri_to_loopback_denied_by_strict_policy` |
| Vérification **après** résolution DNS (toutes les adresses) | `CoreSession::check_uri_policy` (lookup puis `check` sur chaque addr) | `policy.rs` (hôte numérique) + code : `count == 0` → refus |
| Fetch HTTP borné (taille, politique) | `services::fetch_checked` + `read_body_limited` | `services.rs` RSS/checker |
| Mode permissif réservé aux tests | `IpPolicy::permissive()` — jamais en config de production | — |

Points d'entrée HTTP couverts : `add_download` (URI `http(s)`), RSS,
torrent checker (HTTP scrape), `torrentinfo/uri`. Toute URL fournie
par un tiers via l'API passe par `check_uri_policy` **avant** le
moindre trafic.

## 2. Politique de sortie tunnel — `exit_policy.rs`

| Contrôle | Emplacement | Test |
| :--- | :--- | :--- |
| Flags de sortie (`EXIT_BT` etc.) exigés selon le protocole | `exit_policy::is_allowed` — appelé à la sortie **et** au retour (bidirectionnel comme `DataChecker` pyipv8) | `tribler-tunnel::tunnel_exit_drops_non_bt_or_unflagged` |
| `is_allowed` dans les deux sens | `community.rs` exit path + `exit_recv_data` | idem |

## 3. Kill switch — `kill_switch.rs`

| Contrôle | Emplacement | Test |
| :--- | :--- | :--- |
| Engagements **scopés** (`engage_scoped`/`release_scoped`) : le switch reste engagé tant qu'une portée est active — un proxy redevenu joignable ne désarme pas une panne de circuits | `kill_switch.rs` portées `proxy`/`circuits`/`manuel` | `kill_switch::tests::scopes_independants_*` |
| `guard()` refuse add/resume tant qu'engagé | `BtEngine::add`/`resume` appellent `ks.guard()` | `bittorrent::kill_switch_blocks_add_while_proxy_down` |
| Watchdog sonde TCP périodique du proxy (portée `proxy`) | `BtEngine::spawn_proxy_watchdog` | `kill_switch_midtransfer` (proxy mort en plein transfert) |
| Watchdog **circuits** par lane (portée `circuits`) : engage quand la lane perd tous ses circuits `READY` à `hops` sauts après en avoir eu un — **proxy joignable ≠ circuit disponible** | `ipv8_stack::spawn_circuit_watchdog` + `TunnelCommunity::watch_circuits` (notification événementielle, tick 5 s en filet) | `tribler-core::circuit_detruit_bloque_la_lane_sans_fuite` |

## 4. Guard du proxy SOCKS5 — `proxy_guard.rs`

| Contrôle | Emplacement | Test |
| :--- | :--- | :--- |
| Proxy distant refusé au démarrage (pas de repli direct) | `validate_local_socks5_url` dans `BtEngine::start` | `bittorrent::remote_socks5_proxy_rejected` |
| Loopback numérique exigé | idem | idem |

## 5. Hidden seeding — `socks5.rs` + `udp_relay.rs`

| Contrôle | Emplacement | Test |
| :--- | :--- | :--- |
| Adresse `CIRCUIT_ID_PORT` rejetée si le circuit n'est pas RP READY | `socks5.rs` guard | `socks5_rejects_fake_ip_for_non_rp_circuit` |
| Verrouillage du client relay à la première trame | `udp_relay.rs` first-seen | `hidden_seed_udp_relay_roundtrip` |

## 6. Surface de contrôle locale

- API REST bind `127.0.0.1` uniquement (routeur + daemon).
- `api.key` non utilisé : la seule porte d'entrée est loopback (cf.
  `api_rest_mapping.md`, note `settings.api`).

## Vecteurs vérifiés comme fermés

- Proxy SOCKS5 distant → `start` échoue (pas de fallback direct).
- Kill switch engagé → `add`/`resume` refusés.
- Donnée non-BT vers sortie sans `EXIT_BT` → drop des deux côtés.
- Adresse factice `circuit_id` vers circuit non-RP → rejetée.
- `POST /api/versioning/versions/{v}` supprime uniquement un sous-
  répertoire `v*` validé de `state_dir` (pas de traversée de chemin).
- `createtorrent/dryrun` écrit un fichier `.tribler-*-probe` puis le
  supprime — pas d'écriture arbitraire.
