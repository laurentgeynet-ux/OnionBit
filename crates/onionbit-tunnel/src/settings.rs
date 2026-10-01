//! `TunnelSettings` / `HiddenTunnelSettings` / `TriblerTunnelSettings`
//! Python (pyipv8 `community.py` + `hidden_services.py` + Tribler
//! `core/tunnel/community.py`) : tous les reglages du reseau de
//! tunnels, rassembles dans une seule structure — les defauts
//! reproduisent les valeurs officielles.

use std::time::Duration;

use onionbit_ipv8::UdpAddress;

/// `TunnelSettings` pyipv8 (+ extensions `TriblerTunnelSettings`).
///
/// Les champs portent les noms Python et les defauts sont les valeurs
/// officielles de `TunnelSettings`/`TriblerTunnelSettings` — toute
/// divergence est un ecart documente, pas un choix implicite.
#[derive(Debug, Clone)]
pub struct TunnelSettings {
    /// `min_circuits` Python (1) : plancher de circuits par nombre de
    /// sauts dans `circuits_needed` (`monitor_downloads` borne
    /// `clamp(downloads, min, max)`).
    pub min_circuits: usize,
    /// `max_circuits` Python (8) : plafond de `circuits_needed`.
    pub max_circuits: usize,
    /// `max_joined_circuits` Python (100) : relais + sorties max
    /// acceptes (`should_join_circuit`).
    pub max_joined_circuits: usize,
    /// `max_time` Python (1 h) : duree de vie max d'un circuit.
    pub max_time: Duration,
    /// `max_time_ip` Python (24 h) : duree de vie max d'un circuit
    /// `IP_SEEDER` ou d'un exit servant de point d'introduction.
    pub max_time_ip: Duration,
    /// `max_time_inactive` Python (20 s) : inactivite avant
    /// destruction (`remove_* "no activity"`).
    pub max_time_inactive: Duration,
    /// `max_traffic` Python (10 Gio) : octets (up+down) max par objet
    /// de routage avant destruction (`"traffic limit exceeded"`).
    pub max_traffic: u64,
    /// `circuit_timeout` Python (60 s) : delai max de creation d'un
    /// circuit ; le premier saut (`unverified_hop`) peut encore
    /// changer pendant cette fenetre.
    pub circuit_timeout: Duration,
    /// `unstable_timeout` Python (60 s) : fenetre pendant laquelle un
    /// saut nous autorise a changer le saut suivant.
    pub unstable_timeout: Duration,
    /// `next_hop_timeout` Python (10 s) : delai max d'ajout d'un saut.
    /// Sert aussi de borne a `await circuit.ready`.
    pub next_hop_timeout: Duration,
    /// `swarm_lookup_interval` Python (30 s) : cadence des lookups
    /// PEX/DHT d'un swarm cache (`do_peer_discovery`).
    pub swarm_lookup_interval: Duration,
    /// `swarm_connection_limit` Python (15) : connexions e2e max par
    /// swarm avant d'arreter les lookups.
    pub swarm_connection_limit: usize,
    /// `remove_tunnel_delay` Python (5 s) : delai entre `close()` et
    /// le retrait effectif (laisse passer les donnees post-mortem).
    pub remove_tunnel_delay: Duration,
    /// `ping_interval` (`PING_INTERVAL` tunnel.py, 7,5 s) : cadence de
    /// `do_ping` sur les circuits.
    pub ping_interval: Duration,
    /// `peer_flags` Python (`{PEER_FLAG_RELAY, PEER_FLAG_SPEED_TEST}`)
    /// : services annonces aux pairs tunnel (0 = refuser les
    /// `create`).
    pub peer_flags: i32,
    /// `max_relay_early` Python (8) : cellules `relay_early` max
    /// autorisees a traverser un relais. `u32` pour rester comparable
    /// aux compteurs `relay_early_count` non bornes cote Python.
    pub max_relay_early: u32,

    // -- `TriblerTunnelSettings` (core/tunnel/community.py) ---------
    /// `default_hops` Python (0) : sauts par defaut des
    /// telechargements sans `anon_hops` explicite.
    pub default_hops: usize,
    /// `max_intro_points` Python (10) : points d'introduction max
    /// crees pour les swarms seedes.
    pub max_intro_points: usize,

    // -- Parametres du `Swarm` Python (`tunnel.py`) ----------------
    /// `max_ip_age` Python (180 s) : TTL d'un point d'introduction
    /// dans `swarm.intro_points`.
    pub swarm_max_ip_age: Duration,
    /// `min_dht_lookup_interval` Python (300 s) : delai sans reponse
    /// DHT avant de retenter un lookup DHT plutot que PEX.
    pub min_dht_lookup_interval: Duration,
    /// `max_dht_lookup_interval` Python (120 s) : si aucun point
    /// d'introduction connu, un lookup DHT est tente des que ce delai
    /// sans reponse est atteint (inferieur a `min_` dans le code
    /// officiel — fidelement reproduit).
    pub max_dht_lookup_interval: Duration,

    // -- Extension Rust ----------------------------------------------
    /// Point d'introduction impose (`required_ip` de
    /// `create_introduction_point` pyipv8 expose en configuration) :
    /// quand il est defini, tout circuit `IP_SEEDER` se termine sur ce
    /// pair — bancs controles et diagnostic reseau (certains points
    /// d'introduction publics acceptent `establish-intro` sans relayer
    /// le `create-e2e`). `None` = selection automatique (`select_exit`).
    pub intro_point_peer: Option<UdpAddress>,
    /// Sortie imposee des circuits `DATA` (`required_exit` de
    /// `create_circuit` pyipv8 expose en configuration) : quand il est
    /// defini, tout circuit `DATA` se termine sur ce pair — bancs
    /// controles ou le point d'introduction n'est joignable que via un
    /// saut connu (ex. loopback, NAT sans hairpin). Tant que le pair
    /// n'est pas verifie, AUCUN circuit `DATA` n'est cree.
    /// `None` = selection automatique (`select_exit` / `EXIT_BT`).
    pub data_exit_peer: Option<UdpAddress>,
    /// Intervalle entre deux republications DHT des points
    /// d'introduction d'un swarm seede (`reannounce_intro_points`) —
    /// extension Rust : pyipv8 ne re-annonce jamais, une annonce
    /// perdue ou diluee rend le swarm invisible definitivement.
    pub intro_reannounce_interval: Duration,
}

impl Default for TunnelSettings {
    /// Valeurs officielles pyipv8/Tribler.
    fn default() -> Self {
        Self {
            min_circuits: 1,
            max_circuits: 8,
            max_joined_circuits: 100,
            max_time: Duration::from_secs(60 * 60),
            max_time_ip: Duration::from_secs(24 * 60 * 60),
            max_time_inactive: Duration::from_secs(20),
            max_traffic: 10 * 1024_u64.pow(3),
            circuit_timeout: Duration::from_secs(60),
            unstable_timeout: Duration::from_secs(60),
            next_hop_timeout: Duration::from_secs(10),
            swarm_lookup_interval: Duration::from_secs(30),
            swarm_connection_limit: 15,
            remove_tunnel_delay: Duration::from_secs(5),
            ping_interval: Duration::from_millis(7_500),
            peer_flags: onionbit_network_policy::exit_policy::PEER_FLAG_RELAY
                | onionbit_network_policy::exit_policy::PEER_FLAG_SPEED_TEST,
            max_relay_early: 8,
            default_hops: 0,
            max_intro_points: 10,
            swarm_max_ip_age: Duration::from_secs(180),
            min_dht_lookup_interval: Duration::from_secs(300),
            max_dht_lookup_interval: Duration::from_secs(120),
            intro_point_peer: None,
            data_exit_peer: None,
            intro_reannounce_interval: Duration::from_secs(60),
        }
    }
}

impl TunnelSettings {
    /// `circuit_timeout // next_hop_timeout` Python (6) : tentatives
    /// `send_initial_create`/`send_extend` par saut
    /// (`RetryRequestCache.max_tries`).
    pub fn max_tries(&self) -> i32 {
        (self.circuit_timeout.as_secs() / self.next_hop_timeout.as_secs().max(1)) as i32
    }
}
