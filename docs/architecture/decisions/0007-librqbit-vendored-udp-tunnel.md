# ADR-0007 — librqbit vendored + transport UDP tunnel (`data` cells)

Statut : Acceptée (2026-09-29).

## Contexte

Les telechargements anonymes (`hops > 0`) ne demarraient jamais.
L'investigation des logs et la comparaison avec la reference locale
`ipv8-rust-tunnels` (le `.pyd` que les noeuds Tribler reels executent)
ont montre que le protocole filaire des tunnels n'a **pas** de relais
TCP generique :

- `http-request`/`http-response` (msgs 28/29) est **one-shot** : chaque
  cellule `http-request` declenche chez la sortie un `send_tcp_request`
  complet (connexion TCP, envoi de la requete, lecture d'une reponse
  HTTP entiere, retour en chunks bornes). Pas de flux bidirectionnel
  persistant — donc pas de CONNECT SOCKS5 generique, et les trackers
  **HTTPS ne peuvent pas passer** (le `ClientHello` binaire n'est pas
  du HTTP ; limitation identique dans Tribler officiel).
- Le trafic pairs/DHT/trackers-UDP passe par les cellules **`data`**
  (msg 4) vers des sockets UDP de sortie — `DataChecker.could_be_bt`
  (uTP/UDP-tracker/DHT). Tribler configure sa session anonyme en
  consequence : `enable_outgoing_tcp=False`, `enable_outgoing_utp=True`,
  `anonymous_mode`, `force_proxy` — les connexions pairs sont uTP, la
  DHT et les trackers UDP passent en `UDP ASSOCIATE`.

Probleme : `librqbit` 9.0.1 ne sait pas router son trafic UDP par un
proxy SOCKS5 — son proxy est **TCP-only** (`Socks5Stream::connect` pour
les pairs, `reqwest` pour les trackers HTTP), sa socket uTP et sa DHT
bind des sockets UDP reelles, et `UdpTrackerClient` idem. Avec un
proxy configure, `StreamConnector` court-circuitait meme uTP.

## Decision

Vendored + patch de 4 crates rqbit dans `vendor/` (reliées par
`[patch.crates-io]` dans le `Cargo.toml` racine) :

| Crate | Patch |
| :--- | :--- |
| `librqbit-dualstack-sockets` | nouveau trait object-safe `DatagramSocket` (`send_to`/`recv_from`/`bind_addr`) + impl pour `UdpSocket` |
| `librqbit-utp` | re-export de `UtpEnvironment`/`DefaultUtpEnvironment` (prives upstream, necessaires pour nommer `UtpSocket<T, _>`) |
| `librqbit-dht` | `DhtConfig.socket`/`PersistentDht::create` : socket `DatagramSocket` injectable au lieu du bind UDP |
| `librqbit-tracker-comms` | `UdpTrackerClient::new_with_socket` : socket injectable |
| `librqbit` | `ConnectionOptions.utp_socket` (`Arc<dyn UtpConnector>`, impl blanket sur `UtpSocket<T,E>`) ; `DhtSessionConfig.socket` ; `SessionOptions.udp_tracker_socket` ; le proxy SOCKS5 n'est utilise pour les pairs que si `enable_tcp` |

`enable_tcp=false` (lane anonyme) = **aucun transport TCP pour les
pairs**, meme via le proxy — parite `enable_outgoing_tcp=False`. Le
proxy reste utilise pour les trackers HTTP(S) via `reqwest`.

Nouveau `onionbit_tunnel::tunnel_udp_socket` : `TunnelUdpSocket`
implemente `librqbit_utp::Transport` + `DatagramSocket` au-dessus de
`TunnelCommunity::send_data`/`data_rx` :

- envoi : pinning destination -> circuit `READY` (`select_circuit`
  reference), cellules `data` avec `org_address=0.0.0.0:0` ;
- reception : `data_rx` (broadcast) + filtrage par forme de paquet
  (uTP / DHT / tracker UDP — classifieurs `could_be_*` identiques au
  `DataChecker` des sorties) ;
- aucun datagramme UDP reel n'est emis (anti-fuite par construction).

`Ipv8Stack::anon_engine` cree `TunnelUdpSockets` (uTP/DHT/tracker) par
lane et reactive la DHT anonyme (`enable_dht=true`, routee dans le
tunnel) — conforme a Tribler qui demarre la DHT sur les sessions
anonymes.

## Consequences

- `vendor/` (~4 crates) a resynchroniser manuellement aux upgrades de
  librqbit ; les patches sont minimaux et documentes « OnionBit-
  Torrent vendored patch » dans le code.
- Les trackers **HTTPS** ne fonctionnent pas en mode anonyme — meme
  limitation que Tribler officiel. Les trackers `http://` et `udp://`
  passent (one-shot `http-request`, `data` cells).
- La connexion SOCKS5 `CONNECT` est revenue a la semantique one-shot
  (revert du flux continu de `6d2f11c`, non interoperable avec les
  sorties reelles).
- Non-anonyme inchange : `enable_tcp=true`, pas de socket injectee.

## References

- `ipv8-rust-tunnels` : `src/socks5.rs` (`UDPAssociate`,
  `perform_http_request`), `src/util.rs` (`send_tcp_request`),
  `src/routing/exit.rs` (reponses `http-response` en chunks,
  `send_exit_data` UDP).
- `tribler/core/libtorrent/download_manager/download_manager.py` :
  config session anonyme (`anonymous_mode`, `force_proxy`,
  `enable_outgoing_*`).
