// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stack IPv8 optionnelle de la session (etape 15).
//!
//! Equivalent des composants `IPv8`/`DiscoveryCommunity`/
//! `ContentDiscoveryCommunity`/`TunnelCommunity` de
//! `tribler.core.components` : quand `CoreConfig.ipv8.enabled`,
//! `CoreSession` cree un endpoint UDP IPv8, la discovery, la community
//! de decouverte de contenu et — si `enable_anonymity` — la
//! `TunnelCommunity` avec des proxys SOCKS5 par nombre de sauts
//! (`1..=MAX_ANON_HOPS`) desservant chacun un `BtEngine` dedie
//! (uTP-only + SOCKS5, comme les sessions libtorrent anonymes de
//! Tribler).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use onionbit_bittorrent::{BtEngine, EngineConfig};
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_db::Database;
use onionbit_ipv8::content_discovery::{
    ContentDiscoveryCommunity, ContentProvider, HealthInfo, HEALTH_REQUEST_POPULAR,
};
use onionbit_ipv8::dht::DhtCommunity;
use onionbit_ipv8::discovery::DiscoveryCommunity;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::hidden_services::lookup_info_hash;
use onionbit_tunnel::routing::{circuit_id_to_ip, CIRCUIT_ID_PORT};
use onionbit_tunnel::socks5::Socks5Server;
use onionbit_tunnel::tunnel_udp_socket::TunnelUdpSocket;
use onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID;

use crate::error::{CoreError, Result};

/// Fichier de cle IPv8 persiste dans `state_dir` (`ec.pem`-equivalent,
/// format binaire `LibNaCLSK:`).
const IPV8_KEY_FILE: &str = "ipv8_keypair.bin";

/// Nombre de sauts anonymes maximum (`anon_hops` Python : 1..=3).
pub const MAX_ANON_HOPS: usize = 3;

/// Port d'ecoute UDP IPv8 par defaut (fidele a `pyipv8` : 8090).
pub const DEFAULT_IPV8_PORT: u16 = 8090;

/// Noeuds d'amorcage IPv8 officiels (Dispersy / TU Delft),
/// fideles a `DISPERSY_BOOTSTRAPPER` de `pyipv8.ipv8.configuration`.
pub const DEFAULT_BOOTSTRAP_PEERS: &[&str] = &[
    "130.161.119.206:6421",
    "130.161.119.206:6422",
    "131.180.27.155:6423",
    "131.180.27.156:6424",
    "131.180.27.161:6427",
    "131.180.27.161:6521",
    "131.180.27.161:6522",
    "131.180.27.162:6523",
    "131.180.27.162:6524",
    "130.161.119.215:6525",
    "130.161.119.215:6526",
    "130.161.119.201:6527",
    "130.161.119.201:6528",
    "dispersy1.tribler.org:6421",
    "dispersy1.st.tudelft.nl:6421",
    "dispersy2.tribler.org:6422",
    "dispersy2.st.tudelft.nl:6422",
    "dispersy3.tribler.org:6423",
    "dispersy3.st.tudelft.nl:6423",
    "dispersy4.tribler.org:6424",
];

/// Configuration de la stack IPv8 de session.
///
/// `enabled = false` (defaut) : session sans overlay — identique a
/// `ipv8.enabled = false` cote Tribler.
#[derive(Debug, Clone)]
pub struct Ipv8Config {
    /// Active la stack IPv8 (endpoint UDP + discovery + content
    /// discovery + tunnel si `enable_anonymity`).
    pub enabled: bool,
    /// Adresse d'ecoute UDP de l'endpoint (`"0.0.0.0:0"` = port
    /// ephemere, tests ; Tribler utilise un port configure).
    pub listen_addr: String,
    /// Pairs d'amorcage `"host:port"` (bootstrap discovery).
    pub bootstrap_peers: Vec<String>,
    /// Cree la `TunnelCommunity` + les proxys SOCKS5 anonymes
    /// (requis pour `anon_hops > 0` sur les telechargements).
    pub enable_anonymity: bool,
    /// Flags de service annonces aux pairs tunnel
    /// (`PEER_FLAG_*` — `RELAY`/`EXIT_BT`/...).
    pub peer_flags: i32,
    /// Utilise le `community_id` de `TriblerTunnelCommunity` au lieu
    /// du `pyipv8` (interop avec les clients Tribler installes).
    pub onionbit_tunnel_community: bool,
    /// Cree la `DHTDiscoveryCommunity` (`dht_discovery/enabled`
    /// Python — precondition de `DHTDiscoveryComponent`).
    pub enable_dht: bool,
    /// `walker_interval` Python (s) — cadence des strategies de
    /// decouverte ; sert de reference a `DriftMeasurementStrategy`
    /// (`/api/ipv8/asyncio/drift`).
    pub walker_interval: f64,
    /// `TunnelSettings.min_circuits` Python : nombre de circuits
    /// `READY` (ou en cours) maintenus par lane anonyme. Redondance
    /// necessaire — un unique circuit dont le dernier saut annonce
    /// `EXIT_HTTP` sans le servir reellement (pair instable ou
    /// mensonger du reseau public) bloque la lane pour toujours
    /// (`aucun circuit HTTP pret`/timeouts) puisque le watchdog ne
    /// retente jamais tant qu'un circuit `READY` existe.
    pub min_circuits: u32,
    /// `TunnelSettings.max_circuits` Python : borne haute de la cible
    /// du watchdog (`circuits_needed[hops] = clamp(downloads actifs,
    /// min_circuits, max_circuits)` — `monitor_downloads` de
    /// `TriblerTunnelCommunity`).
    pub max_circuits: u32,
    /// `TunnelSettings.max_joined_circuits` Python (defaut 100) :
    /// plafond de jambes de relais + sockets de sortie servies — au-
    /// dela, les `create` entrants sont refuses
    /// (`should_join_circuit`). Borne la charge de relai que le reseau
    /// peut imposer a ce noeud.
    pub max_joined_circuits: usize,
    /// Mode du plafond de debit servi aux autres pairs — relais +
    /// sortie (extension Rust sans equivalent pyipv8) : `-1` =
    /// automatique (`bandwidth` : fraction de l'upload mesure),
    /// `0` = illimite (comportement pyipv8), `>0` = plafond fixe en
    /// octets/s. `TunnelSettings.max_relayed_bps` recoit la valeur
    /// resolue a la construction (`bandwidth.fallback_bps` en auto).
    pub max_relayed_bps: i64,
    /// Parametres de l'estimateur de capacite upload
    /// (`tunnel_community/bandwidth` — utilise quand
    /// `max_relayed_bps = -1`).
    pub bandwidth: crate::daemon_config::BandwidthConfig,
    /// `libtorrent/socks_listen_ports` Python : ports des proxys
    /// SOCKS5 anonymes, index `[hops-1]` (0 = port ephemere attribue
    /// par l'OS, defaut Tribler `[0]*5`).
    pub socks_listen_ports: Vec<u16>,
    /// `content_discovery_community/enabled` Python : cree la
    /// `ContentDiscoveryCommunity` (recherche distante, select).
    pub enable_content_discovery: bool,
    /// Extension Rust : point d'introduction impose pour les circuits
    /// `IP_SEEDER` (`required_ip` de `create_introduction_point`
    /// pyipv8). `None` = selection automatique (`select_exit`).
    pub intro_point_peer: Option<UdpAddress>,
    /// Extension Rust : sortie imposee pour les circuits `DATA`
    /// (`required_exit` de `create_circuit` pyipv8). `None` =
    /// selection automatique `EXIT_BT`.
    pub data_exit_peer: Option<UdpAddress>,
    /// Guard nodes (ADR-0010, experimentale) : premiers sauts
    /// persistants bornant la loterie Sybil des reconstructions.
    /// `false` = selection pyipv8 exacte.
    pub guards_enabled: bool,
    /// Extension Rust (bancs loopback) : estimation WAN initiale.
    /// Sur un mesh 100 % loopback, `destination_address` des
    /// intro-responses est toujours en sous-reseau LAN →
    /// `my_estimated_wan` resterait indetermine et le DHT muet
    /// (`on_node_discovered` refuse tout noeud sans WAN connu —
    /// meme comportement pyipv8). `None` = apprentissage normal.
    pub estimated_wan: Option<UdpAddress>,
    /// `ipv8/interfaces[UDPIPv6]` Python (`ip:port`, ex. `"[::]:8091"`)
    /// — socket UDP secondaire partageant les memes communities
    /// (`DispatcherEndpoint` pyipv8). `None` = IPv4 seul.
    pub listen_addr_v6: Option<String>,
    /// Extension Rust : nombre max de pairs verifies persistes dans
    /// `ipv8_peers` (recharges au demarrage pour un bootstrap rapide —
    /// pyipv8 ne persiste pas son annuaire).
    pub peer_cache_max: usize,
    /// Age max d'un pair cache (s) — au-dela il est expire au chargement
    /// puis supprime par le prune periodique.
    pub peer_cache_max_age_secs: u64,
    /// Intervalle du snapshot `Network` -> `ipv8_peers` (s).
    pub peer_persist_interval_secs: u64,
    /// Extension Rust : TTL du cache des santes publiees par
    /// `healths_for` (s). Les requetes `HEALTH_REQUEST` distantes
    /// arrivent en rafale ; une fraicheur inferieure a cette duree
    /// n'apporte rien et chaque requete SQL bloque la connexion
    /// partagee (mesure : jusqu'a 24 s d'attente mutex sous charge).
    pub content_healths_cache_secs: u64,
    /// Extension Rust : plafond de debit de la socket DHT d'une lane
    /// anonyme, en datagrammes/s sortants (seau a jetons, rafale de
    /// 1 s). `0` = illimite. La DHT mainline tunnelisee d'un magnet
    /// en stall mesurait ~6 400 cellules/s dans le mesh (~200x la
    /// cadence Tribler equivalente — fingerprint + charge exit) ;
    /// le plafond borne le pire cas quelle que soit la cadence
    /// demandee par le client DHT.
    pub anon_dht_rate_pps: u64,
    /// Extension Rust : posture client-only de la DHT anonyme — les
    /// requetes DHT entrantes (non sollicitees, reinjectees par la
    /// socket de sortie) sont ecartees au lieu d'etre servies : la
    /// lane interroge mais n'amplifie pas. `true` recommande.
    pub anon_dht_client_only: bool,
    /// Extension Rust : plafond (s) du backoff exponentiel applique
    /// aux re-lookups `get_peers` sans progres (patch librqbit-dht
    /// vendored). `0` = intervalle fixe de 60 s (comportement brut).
    /// Avec le plafond, un magnet sans swarm decroit vers une cadence
    /// jitteree plafonnee (`discovery degradee`).
    pub anon_dht_backoff_cap_secs: u64,
    /// Extension Rust : TTL (s) des sources WAN "contactees" par une
    /// socket de sortie — `exit_recv_data` ne reencapsule vers
    /// l'amont que les datagrammes non-IPv8 provenant d'une
    /// destination passee par `exit_data` dans cette fenetre
    /// (semantique conntrack). `0` desactive le filtre (comportement
    /// pyipv8 : tout reinjecter). Sans ce filtre le bruit UDP
    /// adresse au port de sortie etait reinjecte puis repondu
    /// (amplification ~6 400 cellules/s mesuree en mesh).
    pub exit_inbound_source_ttl_secs: u64,
    /// Extension Rust : borne de la table des sources contactees par
    /// socket de sortie (`exit_inbound_source_ttl_secs`).
    pub exit_inbound_max_sources: usize,
    /// Extension Rust : intervalle d'echantillonnage des compteurs de
    /// l'endpoint (ms) pour le debit `rate_up`/`rate_down` de
    /// `/api/statistics/ipv8` — calcule cote daemon pour que toutes
    /// les UI (desktop, web) affichent la meme valeur.
    pub stats_rate_sample_ms: u64,
    /// Extension Rust : profondeur (s) de la fenetre glissante du
    /// debit — doit couvrir le sondage UI (~5 s) pour une mesure
    /// non nulle entre deux ticks.
    pub stats_rate_window_secs: u64,
    /// Extension Rust (ADR-0011) : demarre le service messagerie e2e
    /// — le demon joint le swarm `messaging_hash(pk)` (seeder avec
    /// la cle d'identite), maintient des points d'introduction et
    /// accepte les liaisons de contacts. `true` par defaut (bancs
    /// `MS-*` + interop messaging 19/19 valides) ; annonce
    /// `CAP_MSG_V1` dans les `hello` ext quand la communaute tourne.
    /// Necessite `enable_anonymity`.
    pub enable_messaging: bool,
    /// Extension Rust (ADR-0011) : sauts des circuits messagerie
    /// (`hops` de `join_swarm` — meme echelle que `anon_hops` : 1 =
    /// 1 relais + 1 saut intro/rendez-vous automatique).
    pub messaging_hops: usize,
    /// Extension Rust (ADR-0015) : comptabilite locale par pair des
    /// octets servis/utilises sur les tunnels — persistance
    /// `peer_stats` et exposition `/api/ipv8/tunnel/ledger`.
    /// `enforce` = porte d'admission active sur les `create`
    /// entrants (deficit > `max_deficit_bytes` apres la gratuite
    /// `soft_cap`). Local et consultatif : aucun mecanisme filaire.
    pub ledger_enabled: bool,
    /// Porte d'admission (voir `ledger_enabled`). `false` (defaut)
    /// = collection seule.
    pub ledger_enforce: bool,
    /// Gates de consentement messagerie adossees au trust ext
    /// (`kind=identity`, ADR-0015 §6) : `flagged` → score < 0 bloque
    /// sans `pending` ; `endorsed` → score > 0 admis `Active` direct.
    /// `false` par defaut — le consentement manuel reste la regle.
    pub messaging_consent_flagged: bool,
    /// Voir `messaging_consent_flagged` — auto-accept des endorses.
    pub messaging_consent_endorsed: bool,
    /// Gate dette sur l'admission messagerie (deficit ledger >
    /// `max_deficit_bytes` → refuse) — n'opere que si
    /// `ledger_enforce` est actif.
    pub messaging_consent_ledger: bool,
    /// Octets servis gratuitement avant tout refus (periode de
    /// gratuite pour les nouveaux pairs).
    pub ledger_soft_cap: usize,
    /// Deficit `served - used` au-dela duquel un `create` entrant est
    /// refuse (s'il n'y a plus de gratuite).
    pub ledger_max_deficit_bytes: u64,
    /// Cadence de flush `peer_stats` vers SQLite (s).
    pub ledger_tick_secs: u64,
    /// Nombre de comptes conserves en memoire / en base (`0` =
    /// borne par defaut).
    pub ledger_max_peers: usize,
    /// Extension Rust (ADR-0015 §1–2) : cree la `OnionbitExtCommunity`
    /// — communaute OnionBit-only (`hello` lazy vers pairs deja
    /// connus, jamais de walk). `true` en production (`ExtConfig`
    /// et `production()`) : signe en clair comme le discovery IPv8
    /// legacy, borne par `hello_fanout`/`hello_cooldown` — sinon deux
    /// installs OnionBit ne se reconnaissent jamais. `false` dans
    /// `Ipv8Config::default()` (preset neutre « tout off »).
    pub ext_enabled: bool,
    /// Cadence du sondage `hello` ext (s).
    pub ext_hello_interval_secs: u64,
    /// Pairs sondes par tick ext au maximum.
    pub ext_hello_fanout: u32,
    /// Cooldown anti-tempete des `hello` ext par pair (s) — un pair
    /// muet n'est plus sollicite dans cette fenetre.
    pub ext_hello_cooldown_secs: u64,
    /// Curateurs suivis (cles publiques LibNaCl binaires,
    /// `ext/curators` decodees hex) — seules leurs attestations sont
    /// stockees, re-emises et comptees dans le score local.
    pub ext_curators: Vec<Vec<u8>>,
    /// Derive d'horloge toleree sur `ts` des attestations recues (s).
    pub ext_attest_max_future_skew_secs: u64,
    /// Borne de la liste `latest` exposee par l'API ext.
    pub ext_attest_list_max: u32,
    /// Fenetre du budget `ATTEST` par emetteur (s) — borne le cout
    /// du chemin de reception face aux rafales.
    pub ext_attest_rate_window_secs: u64,
    /// Messages `ATTEST` acceptes par emetteur et par fenetre.
    pub ext_attest_rate_max: u32,
    /// Borne memoire de la table de budget `ATTEST` (emetteurs
    /// simultanes suivis).
    pub ext_attest_rate_table_max: u32,
    /// Phase 9c — ledger bilateral signe dans l'extension (propose
    /// et co-signe les liens de reglement). `true` par defaut quand
    /// `ext_enabled` : la mesure tourne ; le *gate* d'admission
    /// reste gouverne par `tunnel_community/ledger_enforce`.
    pub ext_ledger_enabled: bool,
    /// Taille de la tranche sign-then-serve (octets) — au-dela du
    /// delta impaye, aucune nouvelle tranche sans co-signature.
    pub ext_ledger_tranche_bytes: u64,
    /// Cadence du tick de settlement (s).
    pub ext_ledger_settle_interval_secs: u64,
    /// Expiration d'une proposition en vol avant relance (s).
    pub ext_ledger_propose_timeout_secs: u64,
    /// Relances maximales d'une proposition ignoree.
    pub ext_ledger_max_retries: u32,
    /// Derive de mesure toleree entre serveur et beneficiaire
    /// (pour mille) — les deux comptent le meme flot depuis deux
    /// points de mesure.
    pub ext_ledger_drift_permille: u64,
    /// Plancher absolu de la derive toleree (octets).
    pub ext_ledger_drift_min_bytes: u64,
    /// Fenetre du budget `LEDGER_*` par emetteur (s).
    pub ext_ledger_rate_window_secs: u64,
    /// Messages `LEDGER_*` acceptes par emetteur et par fenetre.
    pub ext_ledger_rate_max: u32,
    /// Borne memoire de la table de budget `LEDGER_*`.
    pub ext_ledger_rate_table_max: u32,
    /// Capacite du store de liens (memoire sans persistance).
    pub ext_ledger_store_max: u32,
    /// Tetes poussees par tick vers des pairs ext (gossip borne).
    pub ext_ledger_head_fanout: u32,
    /// Phase 9e — enveloppes OBF negociees (opt-in, ADR-0015 §7).
    pub ext_obf_enabled: bool,
    /// Classe de padding OBF (octets).
    pub ext_obf_pad_bucket: u32,
    /// Jitter du sondage `hello` (% de l'intervalle).
    pub ext_hello_jitter_pct: u32,
}

impl Ipv8Config {
    /// Configuration pour une session connectee au reseau reel :
    /// IPv8 actif sur le port configure (ou 8090 par defaut),
    /// noeuds d'amorcage officiels, tunnels actifs avec l'identifiant
    /// de communaute Tribler officiel.
    pub fn production() -> Self {
        Self {
            enabled: true,
            listen_addr: format!("0.0.0.0:{DEFAULT_IPV8_PORT}"),
            bootstrap_peers: DEFAULT_BOOTSTRAP_PEERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            enable_anonymity: true,
            peer_flags: onionbit_network_policy::exit_policy::PEER_FLAG_RELAY
                | onionbit_network_policy::exit_policy::PEER_FLAG_SPEED_TEST,
            onionbit_tunnel_community: true,
            enable_dht: true,
            walker_interval: DEFAULT_WALKER_INTERVAL,
            min_circuits: DEFAULT_MIN_CIRCUITS,
            max_circuits: DEFAULT_MAX_CIRCUITS,
            max_joined_circuits: DEFAULT_MAX_JOINED_CIRCUITS,
            max_relayed_bps: RELAY_RATE_AUTO,
            bandwidth: crate::daemon_config::BandwidthConfig::default(),
            socks_listen_ports: vec![0; MAX_ANON_HOPS],
            enable_content_discovery: true,
            intro_point_peer: None,
            data_exit_peer: None,
            guards_enabled: true,
            estimated_wan: None,
            listen_addr_v6: Some(format!("[::]:{}", DEFAULT_IPV8_PORT + 1)),
            peer_cache_max: DEFAULT_PEER_CACHE_MAX,
            peer_cache_max_age_secs: DEFAULT_PEER_CACHE_MAX_AGE_SECS,
            peer_persist_interval_secs: DEFAULT_PEER_PERSIST_INTERVAL_SECS,
            content_healths_cache_secs: DEFAULT_CONTENT_HEALTHS_CACHE_SECS,
            anon_dht_rate_pps: DEFAULT_ANON_DHT_RATE_PPS,
            anon_dht_client_only: true,
            anon_dht_backoff_cap_secs: DEFAULT_ANON_DHT_BACKOFF_CAP_SECS,
            exit_inbound_source_ttl_secs: DEFAULT_EXIT_INBOUND_TTL_SECS,
            exit_inbound_max_sources: DEFAULT_EXIT_INBOUND_MAX_SOURCES,
            stats_rate_sample_ms: DEFAULT_STATS_RATE_SAMPLE_MS,
            stats_rate_window_secs: DEFAULT_STATS_RATE_WINDOW_SECS,
            // Meme defaut que `TunnelCommunityConfig::default()` :
            // messagerie ADR-0011 active — annoncee via `CAP_MSG_V1`
            // du hello ext (ADR-0015).
            enable_messaging: true,
            messaging_hops: DEFAULT_MESSAGING_HOPS,
            messaging_consent_flagged: false,
            messaging_consent_endorsed: false,
            messaging_consent_ledger: false,
            ledger_enabled: true,
            ledger_enforce: false,
            ledger_soft_cap: DEFAULT_LEDGER_SOFT_CAP as usize,
            ledger_max_deficit_bytes: DEFAULT_LEDGER_MAX_DEFICIT_BYTES,
            ledger_tick_secs: DEFAULT_LEDGER_TICK_SECS,
            ledger_max_peers: DEFAULT_LEDGER_MAX_PEERS as usize,
            // Meme defaut que `ExtConfig::default()` : la communaute
            // ext tourne en production (ADR-0015 — interconnexion
            // OnionBit<->OnionBit sinon impossible).
            ext_enabled: true,
            ext_hello_interval_secs: DEFAULT_EXT_HELLO_INTERVAL_SECS,
            ext_hello_fanout: DEFAULT_EXT_HELLO_FANOUT,
            ext_hello_cooldown_secs: DEFAULT_EXT_HELLO_COOLDOWN_SECS,
            ext_curators: Vec::new(),
            ext_attest_max_future_skew_secs: DEFAULT_EXT_ATTEST_MAX_FUTURE_SKEW_SECS,
            ext_attest_list_max: DEFAULT_EXT_ATTEST_LIST_MAX,
            ext_attest_rate_window_secs: DEFAULT_EXT_ATTEST_RATE_WINDOW_SECS,
            ext_attest_rate_max: DEFAULT_EXT_ATTEST_RATE_MAX,
            ext_attest_rate_table_max: DEFAULT_EXT_ATTEST_RATE_TABLE_MAX,
            ext_ledger_enabled: true,
            ext_ledger_tranche_bytes: DEFAULT_EXT_LEDGER_TRANCHE_BYTES,
            ext_ledger_settle_interval_secs: DEFAULT_EXT_LEDGER_SETTLE_SECS,
            ext_ledger_propose_timeout_secs: DEFAULT_EXT_LEDGER_TIMEOUT_SECS,
            ext_ledger_max_retries: DEFAULT_EXT_LEDGER_MAX_RETRIES,
            ext_ledger_drift_permille: DEFAULT_EXT_LEDGER_DRIFT_PERMILLE,
            ext_ledger_drift_min_bytes: DEFAULT_EXT_LEDGER_DRIFT_MIN_BYTES,
            ext_ledger_rate_window_secs: DEFAULT_EXT_LEDGER_RATE_WINDOW_SECS,
            ext_ledger_rate_max: DEFAULT_EXT_LEDGER_RATE_MAX,
            ext_ledger_rate_table_max: DEFAULT_EXT_LEDGER_RATE_TABLE_MAX,
            ext_ledger_store_max: DEFAULT_EXT_LEDGER_STORE_MAX,
            ext_ledger_head_fanout: DEFAULT_EXT_LEDGER_HEAD_FANOUT,
            ext_obf_enabled: false,
            ext_obf_pad_bucket: DEFAULT_EXT_OBF_PAD_BUCKET,
            ext_hello_jitter_pct: DEFAULT_EXT_HELLO_JITTER_PCT,
        }
    }
}

impl Default for Ipv8Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: "0.0.0.0:0".into(),
            bootstrap_peers: Vec::new(),
            enable_anonymity: false,
            peer_flags: onionbit_network_policy::exit_policy::PEER_FLAG_RELAY,
            onionbit_tunnel_community: false,
            // `dht_discovery.enabled` vaut `true` par defaut dans
            // `tribler_config` — la community n'est creee que si
            // `enabled` est vrai de toute facon.
            enable_dht: true,
            walker_interval: DEFAULT_WALKER_INTERVAL,
            min_circuits: DEFAULT_MIN_CIRCUITS,
            max_circuits: DEFAULT_MAX_CIRCUITS,
            max_joined_circuits: DEFAULT_MAX_JOINED_CIRCUITS,
            max_relayed_bps: RELAY_RATE_AUTO,
            bandwidth: crate::daemon_config::BandwidthConfig::default(),
            socks_listen_ports: vec![0; MAX_ANON_HOPS],
            enable_content_discovery: true,
            intro_point_peer: None,
            data_exit_peer: None,
            guards_enabled: true,
            estimated_wan: None,
            listen_addr_v6: None,
            peer_cache_max: DEFAULT_PEER_CACHE_MAX,
            peer_cache_max_age_secs: DEFAULT_PEER_CACHE_MAX_AGE_SECS,
            peer_persist_interval_secs: DEFAULT_PEER_PERSIST_INTERVAL_SECS,
            content_healths_cache_secs: DEFAULT_CONTENT_HEALTHS_CACHE_SECS,
            anon_dht_rate_pps: DEFAULT_ANON_DHT_RATE_PPS,
            anon_dht_client_only: true,
            anon_dht_backoff_cap_secs: DEFAULT_ANON_DHT_BACKOFF_CAP_SECS,
            exit_inbound_source_ttl_secs: DEFAULT_EXIT_INBOUND_TTL_SECS,
            exit_inbound_max_sources: DEFAULT_EXIT_INBOUND_MAX_SOURCES,
            stats_rate_sample_ms: DEFAULT_STATS_RATE_SAMPLE_MS,
            stats_rate_window_secs: DEFAULT_STATS_RATE_WINDOW_SECS,
            enable_messaging: false,
            messaging_hops: DEFAULT_MESSAGING_HOPS,
            messaging_consent_flagged: false,
            messaging_consent_endorsed: false,
            messaging_consent_ledger: false,
            ledger_enabled: true,
            ledger_enforce: false,
            ledger_soft_cap: DEFAULT_LEDGER_SOFT_CAP as usize,
            ledger_max_deficit_bytes: DEFAULT_LEDGER_MAX_DEFICIT_BYTES,
            ledger_tick_secs: DEFAULT_LEDGER_TICK_SECS,
            ledger_max_peers: DEFAULT_LEDGER_MAX_PEERS as usize,
            // Preset neutre « tout off » (ipv8.enabled=false aussi) :
            // le defaut produit vit dans `ExtConfig`/`production()`.
            ext_enabled: false,
            ext_hello_interval_secs: DEFAULT_EXT_HELLO_INTERVAL_SECS,
            ext_hello_fanout: DEFAULT_EXT_HELLO_FANOUT,
            ext_hello_cooldown_secs: DEFAULT_EXT_HELLO_COOLDOWN_SECS,
            ext_curators: Vec::new(),
            ext_attest_max_future_skew_secs: DEFAULT_EXT_ATTEST_MAX_FUTURE_SKEW_SECS,
            ext_attest_list_max: DEFAULT_EXT_ATTEST_LIST_MAX,
            ext_attest_rate_window_secs: DEFAULT_EXT_ATTEST_RATE_WINDOW_SECS,
            ext_attest_rate_max: DEFAULT_EXT_ATTEST_RATE_MAX,
            ext_attest_rate_table_max: DEFAULT_EXT_ATTEST_RATE_TABLE_MAX,
            ext_ledger_enabled: true,
            ext_ledger_tranche_bytes: DEFAULT_EXT_LEDGER_TRANCHE_BYTES,
            ext_ledger_settle_interval_secs: DEFAULT_EXT_LEDGER_SETTLE_SECS,
            ext_ledger_propose_timeout_secs: DEFAULT_EXT_LEDGER_TIMEOUT_SECS,
            ext_ledger_max_retries: DEFAULT_EXT_LEDGER_MAX_RETRIES,
            ext_ledger_drift_permille: DEFAULT_EXT_LEDGER_DRIFT_PERMILLE,
            ext_ledger_drift_min_bytes: DEFAULT_EXT_LEDGER_DRIFT_MIN_BYTES,
            ext_ledger_rate_window_secs: DEFAULT_EXT_LEDGER_RATE_WINDOW_SECS,
            ext_ledger_rate_max: DEFAULT_EXT_LEDGER_RATE_MAX,
            ext_ledger_rate_table_max: DEFAULT_EXT_LEDGER_RATE_TABLE_MAX,
            ext_ledger_store_max: DEFAULT_EXT_LEDGER_STORE_MAX,
            ext_ledger_head_fanout: DEFAULT_EXT_LEDGER_HEAD_FANOUT,
            ext_obf_enabled: false,
            ext_obf_pad_bucket: DEFAULT_EXT_OBF_PAD_BUCKET,
            ext_hello_jitter_pct: DEFAULT_EXT_HELLO_JITTER_PCT,
        }
    }
}

/// Lane anonyme : proxy SOCKS5 + moteur dedies a un nombre de sauts
/// (equivalent des sessions libtorrent `hops=1..3` de Tribler).
struct AnonLane {
    /// Port du proxy SOCKS5 (loopback) de cette lane.
    pub socks_addr: SocketAddr,
    /// Garde le serveur SOCKS5 en vie ; `shutdown()` ferme son
    /// listener a la destruction de la lane.
    socks: Arc<Socks5Server>,
    /// Moteur BitTorrent route via le SOCKS5 (uTP only).
    pub engine: BtEngine,
    /// Transport uTP tunnelse de la lane — cote seeder d'un hidden
    /// service, les cellules `data` d'un circuit e2e `RP_SEEDER` lie
    /// y sont injectees (`inject_incoming` + `pin_circuit`).
    pub utp_transport: TunnelUdpSocket,
    /// Socket DHT tunnelsee de la lane — exposee pour les compteurs
    /// `stats()` (observabilite budget/cadence DHT anonyme).
    pub dht_socket: TunnelUdpSocket,
    /// Arret du watchdog de circuits de la lane.
    circuit_watchdog_stop: Arc<tokio::sync::watch::Sender<bool>>,
}

/// Intervalle de sondage des circuits `READY` de la lane — filet de
/// securite seulement : la reaction principale est evenementielle via
/// [`TunnelCommunity::watch_circuits`], sinon une transition
/// `READY -> detruit` plus rapide qu'un tick passerait inapercue.
const CIRCUIT_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Tick de `PingChurn.take_step` (strategies pyipv8 : toutes les
/// 0,5 s ; les cadences reelles des pings sont gatees en interne par
/// `PING_INTERVAL`).
const DHT_STEP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// `register_task("node_maintenance", interval=60)` Python.
const DHT_NODE_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
/// `register_task("value_maintenance", interval=3600)` Python.
const DHT_VALUE_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);
/// `register_task("token_maintenance", interval=300)` Python.
const DHT_TOKEN_MAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// `walker_interval` par defaut (`ipv8.walker_interval` Tribler : 0,5 s).
pub const DEFAULT_WALKER_INTERVAL: f64 = 0.5;
/// `TunnelSettings.min_circuits` par defaut (pyipv8/Tribler et
/// `TunnelCommunityConfig::default()`).
pub const DEFAULT_MIN_CIRCUITS: u32 = 3;

/// `TunnelSettings.max_circuits` pyipv8 (`tribler_config`
/// `tunnel_community/max_circuits`).
pub const DEFAULT_MAX_CIRCUITS: u32 = 8;

/// `TunnelSettings.max_joined_circuits` pyipv8 (`tunnel.py` :
/// `should_join_circuit` refuse au-dela — valeur par defaut 100).
pub const DEFAULT_MAX_JOINED_CIRCUITS: usize = 100;

/// Sentinelle `tunnel_community/max_relayed_rate` : plafond servi
/// automatique — AIMD du `CongestionController` sur le retard de
/// file mesuré par ping des pairs (`bandwidth/*`),
/// `bandwidth/fallback_bps` avant le premier échantillon.
pub const RELAY_RATE_AUTO: i64 = -1;

/// Intervalle par defaut d'echantillonnage des compteurs endpoint
/// pour `rate_up`/`rate_down` (`/api/statistics/ipv8`, extension
/// Rust).
pub const DEFAULT_STATS_RATE_SAMPLE_MS: u64 = 1_000;

/// Profondeur par defaut (s) de la fenetre glissante du debit
/// endpoint — couvre le sondage UI (5 s) avec une marge.
pub const DEFAULT_STATS_RATE_WINDOW_SECS: u64 = 6;
/// Extension Rust (ADR-0011) : sauts par defaut des circuits
/// messagerie — 1 saut de swarm (+1 automatique `IP_SEEDER`/
/// `RP_DOWNLOADER`), comme un telechargement `anon_hops=1`.
pub const DEFAULT_MESSAGING_HOPS: usize = 1;

/// Debit servi applique a la construction de la community quand le
/// mode est `-1` (auto) — `bandwidth/fallback_bps`, remplace a chaud
/// des le premier tick du controleur de congestion.
pub fn initial_relay_bps(mode: i64, bandwidth: &crate::daemon_config::BandwidthConfig) -> u64 {
    if mode < 0 {
        bandwidth.fallback_bps
    } else {
        mode as u64
    }
}

/// Taille max du cache de pairs persists (`ipv8_peers`) — extension
/// Rust : pyipv8 ne persiste pas `Network`, on borne a des centaines
/// d'entrees, les plus fraichement vues.
pub const DEFAULT_PEER_CACHE_MAX: usize = 512;
/// Retention d'un pair cache (7 jours) : au-dela, l'adresse est
/// vraisemblablement reattribute (NAT dynamique).
pub const DEFAULT_PEER_CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// Cadence du snapshot `Network` -> `ipv8_peers` (2 min) — un lot par
/// intervalle plutot qu'une ecriture par pair decouvert.
pub const DEFAULT_PEER_PERSIST_INTERVAL_SECS: u64 = 120;
/// TTL du cache `healths_for` (30 s) — assez court pour rester
/// pertinent, assez long pour absorber les rafales de requetes.
pub const DEFAULT_CONTENT_HEALTHS_CACHE_SECS: u64 = 30;
/// Plafond par defaut de la socket DHT d'une lane anonyme
/// (datagrammes/s sortants). ~30 pps couvre bootstrap et passes
/// `get_peers` (~8-32 datagrammes par vague) tout en bornant le
/// pire cas a quelques Ko/s de cellules — deux ordres de grandeur
/// sous le flux mesure en mesh sans discipline (~6 400 cellules/s
/// pour un magnet en stall).
pub const DEFAULT_ANON_DHT_RATE_PPS: u64 = 30;
/// Plafond par defaut du backoff `get_peers` anonyme (15 min) : un
/// infohash sans progres retombe a ~1 vague de requetes par quart
/// d'heure avec jitter — cadence basse permanente au lieu d'une
/// boucle a plein regime. `0` desactive (intervalle fixe librqbit).
pub const DEFAULT_ANON_DHT_BACKOFF_CAP_SECS: u64 = 15 * 60;
/// TTL par defaut (s) des sources WAN contactees par une socket de
/// sortie (filtre conntrack de `exit_recv_data`) — couvre les
/// reponses DHT/tracker/uTP (timeouts ~10-15 s) et les echanges e2e
/// espaces, sans laisser le bruit WAN rentrer dans le tunnel.
pub const DEFAULT_EXIT_INBOUND_TTL_SECS: u64 = 300;
/// Borne par defaut de la table des sources contactees par socket de
/// sortie — une lane qui interroge des milliers de noeuds DHT ne fait
/// pas croitre la table sans limite.
pub const DEFAULT_EXIT_INBOUND_MAX_SOURCES: usize = 2048;

/// Seuil `joined` par defaut de la gate d'admission du ledger
/// (ADR-0015) : la pression est mesuree sur les objets de routage
/// servis ; sous ce seuil, tout `create` est admis (pyipv8 exact).
/// 80 < `DEFAULT_MAX_JOINED_CIRCUITS` (100) — la gate est active
/// avant que la borne dure ne sature.
pub const DEFAULT_LEDGER_SOFT_CAP: u32 = 80;
/// Credit de demarrage / dette maximale toleree par pair quand la
/// gate est active (octets). 256 Mio ≈ un telechargement modeste en
/// 3 sauts — assez pour qu'un nouveau pair prouve sa bonne foi, pas
/// assez pour un free-riding durable.
pub const DEFAULT_LEDGER_MAX_DEFICIT_BYTES: u64 = 256 * 1024 * 1024;
/// Cadence de comptage par deltas + flush `peer_stats` (s).
pub const DEFAULT_LEDGER_TICK_SECS: u64 = 30;
/// Borne de la table `peer_stats` — au-dela les nouvelles cles ne
/// sont plus suivies (protection contre un flot de cles fraiches).
pub const DEFAULT_LEDGER_MAX_PEERS: u32 = 8192;

/// Cadence par defaut du sondage `hello` ext (ADR-0015 §2).
pub const DEFAULT_EXT_HELLO_INTERVAL_SECS: u64 = 60;
/// Pairs sondes par tick ext au maximum (fanout borne — le `hello`
/// est un signal observable, on minimise son exposition).
pub const DEFAULT_EXT_HELLO_FANOUT: u32 = 5;
/// Cooldown par defaut des `hello` ext par pair : un pair muet
/// (Tribler) n'est plus sollicite pendant 1 h.
pub const DEFAULT_EXT_HELLO_COOLDOWN_SECS: u64 = 3600;
/// Derive d'horloge toleree par defaut sur `ts` des attestations
/// ext (10 min — au-dela, le « latest wins » serait gagne pour
/// toujours).
pub const DEFAULT_EXT_ATTEST_MAX_FUTURE_SKEW_SECS: u64 = 600;
/// Borne par defaut de la liste `latest` exposee par l'API ext.
pub const DEFAULT_EXT_ATTEST_LIST_MAX: u32 = 256;
/// Fenetre par defaut du budget `ATTEST` par emetteur (s).
pub const DEFAULT_EXT_ATTEST_RATE_WINDOW_SECS: u64 = 60;
/// Messages `ATTEST` par emetteur et par fenetre (large : un
/// backfill legitime peut en relayer des centaines d'un coup).
pub const DEFAULT_EXT_ATTEST_RATE_MAX: u32 = 256;
/// Borne memoire par defaut de la table de budget `ATTEST`.
pub const DEFAULT_EXT_ATTEST_RATE_TABLE_MAX: u32 = 4096;
/// Tranche sign-then-serve par defaut : 16 Mio — la taille de la
/// reglette de credit entre deux reglements co-signes.
pub const DEFAULT_EXT_LEDGER_TRANCHE_BYTES: u64 = 16 * 1024 * 1024;
/// Cadence du tick de settlement (s).
pub const DEFAULT_EXT_LEDGER_SETTLE_SECS: u64 = 60;
/// Expiration d'une proposition en vol (s).
pub const DEFAULT_EXT_LEDGER_TIMEOUT_SECS: u64 = 120;
/// Relances maximales d'une proposition.
pub const DEFAULT_EXT_LEDGER_MAX_RETRIES: u32 = 3;
/// Derive de mesure toleree, pour mille (12,5%).
pub const DEFAULT_EXT_LEDGER_DRIFT_PERMILLE: u64 = 125;
/// Plancher absolu de la derive (256 Kio).
pub const DEFAULT_EXT_LEDGER_DRIFT_MIN_BYTES: u64 = 256 * 1024;
/// Fenetre du budget `LEDGER_*` par emetteur (s).
pub const DEFAULT_EXT_LEDGER_RATE_WINDOW_SECS: u64 = 60;
/// Messages `LEDGER_*` par emetteur et par fenetre.
pub const DEFAULT_EXT_LEDGER_RATE_MAX: u32 = 128;
/// Borne memoire de la table de budget `LEDGER_*`.
pub const DEFAULT_EXT_LEDGER_RATE_TABLE_MAX: u32 = 4096;
/// Capacite du store de liens.
pub const DEFAULT_EXT_LEDGER_STORE_MAX: u32 = 65536;
/// Fanout des tetes de chaine gossip par tick.
pub const DEFAULT_EXT_LEDGER_HEAD_FANOUT: u32 = 3;
/// Phase 9e — enveloppes OBF off par defaut (opt-in, ADR-0015 §7).
pub const DEFAULT_EXT_OBF_PAD_BUCKET: u32 = 256;
/// Jitter `hello` par defaut : 25 % de l'intervalle.
pub const DEFAULT_EXT_HELLO_JITTER_PCT: u32 = 25;

/// Tache de maintenance DHT (`PingChurn.take_step` +
/// `node_maintenance`/`value_maintenance`/`token_maintenance` +
/// propagation de `my_estimated_wan` depuis la discovery, car Python
/// partage le meme `my_peer` entre communautes). Retourne l'arret.
fn spawn_dht_maintenance(
    dht: Arc<DhtCommunity>,
    discovery: Arc<DiscoveryCommunity>,
    tasks: crate::asyncio::TaskRegistry,
) -> tokio::sync::watch::Sender<bool> {
    // `register_task(...)` de `DHTCommunity.unload` Python — les
    // trois taches partagent la meme boucle ici mais restent
    // declarees separement dans le registre (`/asyncio/tasks`).
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "node_maintenance",
        Some(DHT_NODE_MAINT_INTERVAL.as_secs_f64()),
    );
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "value_maintenance",
        Some(DHT_VALUE_MAINT_INTERVAL.as_secs_f64()),
    );
    tasks.register(
        Some("DHTDiscoveryCommunity"),
        "token_maintenance",
        Some(DHT_TOKEN_MAINT_INTERVAL.as_secs_f64()),
    );
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        // Premier `token_maintenance` immediat (community.py Python :
        // appele avant le register_task, au cas ou des requetes
        // arriveraient avant le premier tick).
        dht.token_maintenance();
        let mut step = tokio::time::interval(DHT_STEP_INTERVAL);
        let mut node = tokio::time::interval(DHT_NODE_MAINT_INTERVAL);
        let mut value = tokio::time::interval(DHT_VALUE_MAINT_INTERVAL);
        let mut token = tokio::time::interval(DHT_TOKEN_MAINT_INTERVAL);
        loop {
            tokio::select! {
                _ = rx.changed() => break,
                _ = step.tick() => dht.step(),
                _ = node.tick() => dht.node_maintenance().await,
                _ = value.tick() => dht.value_maintenance(),
                _ = token.tick() => dht.token_maintenance(),
            }
            // `my_peer` Python est partage entre Discovery et DHT :
            // quand l'introduction apprend notre WAN, le DHT le voit.
            let wan = discovery.my_estimated_wan();
            if !wan.is_unspecified() {
                dht.set_my_wan(wan);
            }
            let lan = discovery.my_estimated_lan();
            if !lan.is_unspecified() {
                dht.set_my_lan(lan);
            }
        }
        tracing::debug!("maintenance DHT arretee");
    });
    tx
}

/// Watchdog de circuits d'une lane anonyme : la lane est
/// **indisponible tant qu'aucun circuit `DATA` `READY` a `hops` sauts
/// n'existe** — la portee `"circuits"` est engagee des la creation
/// (fail-closed : un `add`/`resume` avant le premier circuit est
/// refuse par `guard()` au lieu d'attendre un CONNECT voue a
/// l'echec) et tant que `ready_data_circuits_of_hops(hops)` est vide
/// — le meme predicat que la selection de circuit donnees du serveur
/// SOCKS5 et des sockets UDP tunnel : un circuit `IP_*`/`RP_*` ou
/// d'un autre nombre de sauts ne desarme pas cette lane.
///
/// Distinct du watchdog proxy du moteur : le listener SOCKS5 local
/// peut rester joignable alors que tous les circuits sont morts —
/// `proxy joignable != circuit disponible`.
fn spawn_circuit_watchdog(
    tunnel: Arc<TunnelCommunity>,
    hops: usize,
    ks: Arc<onionbit_network_policy::kill_switch::KillSwitch>,
    circuit_bounds: Arc<(AtomicUsize, AtomicUsize)>,
    engine: BtEngine,
) -> Arc<tokio::sync::watch::Sender<bool>> {
    // Fail-closed des la creation de la lane (avant tout circuit).
    ks.engage_scoped(
        "circuits",
        format!("aucun circuit DATA READY a {hops} sauts"),
    );
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let mut changes = tunnel.watch_circuits();
    tokio::spawn(async move {
        // `monitor_downloads` de `TriblerTunnelCommunity` :
        // `circuits_needed[hops] = clamp(downloads actifs,
        // min_circuits, max_circuits)`. Ecart assumé : la lane garde
        // `min_circuits` en veille meme sans telechargement actif
        // (Python ne demande des circuits qu'aux hops utilises) — un
        // premier `add` n'attend pas la construction a froid.
        let circuits_needed = |engine: &BtEngine| {
            let actifs = engine
                .list()
                .iter()
                .filter(|d| {
                    matches!(
                        d.state,
                        onionbit_bittorrent::DownloadState::Downloading
                            | onionbit_bittorrent::DownloadState::Seeding
                            | onionbit_bittorrent::DownloadState::Initializing
                    )
                })
                .count();
            // Bornes relues a chaud (`POST /api/settings` ->
            // `set_circuit_bounds`), comme `self.settings` Python.
            actifs.clamp(
                circuit_bounds.0.load(Ordering::Relaxed),
                circuit_bounds.1.load(Ordering::Relaxed).max(1),
            )
        };
        let mut tick = tokio::time::interval(CIRCUIT_PROBE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Tentative immediate de construction de circuit(s). Plusieurs
        // circuits en parallele (`min_circuits`, `TunnelSettings`
        // pyipv8) sont necessaires : un unique circuit dont le
        // dernier saut annonce `EXIT_HTTP` sans le servir reellement
        // (pair instable/mensonger du reseau public) bloque
        // definitivement la lane sinon, faute d'alternative pour le
        // selecteur SOCKS5 (`select_http_circuit`).
        let _ = tunnel
            .build_circuits_if_needed(hops, circuits_needed(&engine))
            .await;
        // Premier tick immediat : absorbe (l'evaluation initiale est
        // deja faite ci-dessus).
        tick.tick().await;
        loop {
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {
                    let _ = tunnel
                        .build_circuits_if_needed(hops, circuits_needed(&engine))
                        .await;
                }
            }
            if tunnel.ready_data_circuits_of_hops(hops).is_empty() {
                ks.engage_scoped(
                    "circuits",
                    format!("aucun circuit DATA READY a {hops} sauts"),
                );
            } else {
                ks.release_scoped("circuits");
            }
        }
    });
    Arc::new(stop_tx)
}

/// `ContentProvider` adosse a la base : santes depuis `torrent_state`,
/// metadonnees depuis `channel_node`, serialisation `.mdblob` + LZ4.
struct SessionContentProvider {
    db: Arc<Database>,
    /// `Notification.torrent_metadata_added` Python : chaque entree
    /// inseree est notifiee (nourrit l'augmenteur de recherche).
    notifier: crate::notifier::Notifier,
    /// `remote_queries_in_progress` Python : une seule requete
    /// `txt_filter` distante a la fois (`process_rpc_query_rate_limited`).
    remote_queries_in_progress: std::sync::atomic::AtomicUsize,
    /// `max_response_size` Python (100) : entrees max par select.
    max_response_size: usize,
    /// `maximum_payload_size` Python (1300) : octets d'entrees max
    /// par chunk de reponse.
    max_payload_size: usize,
    /// Cache des santes publiees (`healths_cache_ttl` : la jointure
    /// `channel_node`/`torrent_state` n'a pas besoin d'une fraicheur
    /// inferieure a cette duree).
    healths_cache: Mutex<HashMap<u8, (std::time::Instant, Vec<HealthInfo>)>>,
    /// TTL du cache ci-dessus (`Ipv8Config::content_healths_cache_secs`).
    healths_cache_ttl: std::time::Duration,
    /// Dedup memoire des entrees recues (`public_key`, `id_`) —
    /// remplace la dedup persistee `channel::insert` : les resultats
    /// de recherche distants ne sont plus stockes en base, seuls les
    /// nouveaux objets de la session sont notifies a l'UI.
    /// Borne [`SEEN_NODES_CAP`].
    seen_nodes: Mutex<std::collections::HashSet<(Vec<u8>, i64)>>,
    /// Memoire gossip : santes connues + infohashes affiches.
    gossip: Mutex<GossipMemory>,
}

/// Memoire gossip du provider (tout est volatil, jamais persistee).
///
/// `healths` remplace l'ancien `seen_health_infohashes` (HashSet) :
/// les santes gossip (`seeders`/`leechers`/`last_check`) sont gardees
/// en memoire pour remplir `remote_query_results` et servir de repli
/// a `/api/metadata/torrents/{ih}/health` — equivalent du `torrent_state`
/// Python, mais sans ecriture disque. Bornee [`GOSSIP_HEALTH_CAP`].
///
/// `displayed` retient les infohashes des entrees deja poussees a
/// l'UI (`remote_query_results`) : seuls ces torrents meritent un
/// `TorrentHealthUpdated` quand une sante gossip arrive ensuite —
/// sinon chaque tick de gossip emettrait des centaines d'events SSE
/// pour des torrents que personne n'affiche. Borne [`DISPLAYED_CAP`].
#[derive(Default)]
struct GossipMemory {
    /// `infohash -> (seeders, leechers, last_check)` du dernier gossip.
    healths: std::collections::HashMap<[u8; 20], (u32, u32, u64)>,
    /// Infohashes des resultats deja notifies a l'UI.
    displayed: std::collections::HashSet<[u8; 20]>,
}

/// Borne des santes gossip conservees (~50 octets/entree, 200k ≈ 10 Mio).
/// Au-dela : eviction des entrees perimees (>24 h), puis des plus
/// vieilles si encore plein — la dedup de resolution reste exacte tant
/// que la map n'a jamais atteint la borne.
const GOSSIP_HEALTH_CAP: usize = 200_000;
/// Borne des infohashes suivis pour `TorrentHealthUpdated` (les
/// resultats reels affiches sont tres en deca).
const DISPLAYED_CAP: usize = 50_000;
/// Borne de la dedup `seen_nodes` (croissance non bornee en session
/// longue sinon ; a cette taille un `clear` re-notifie quelques
/// doublons — sans consequence).
const SEEN_NODES_CAP: usize = 300_000;

impl GossipMemory {
    /// Insertion bornee d'une sante gossip — evince a la borne.
    fn insert_health(&mut self, h: &HealthInfo) -> bool {
        if self.healths.len() >= GOSSIP_HEALTH_CAP {
            let cutoff = h.last_check.saturating_sub(86_400);
            self.healths.retain(|_, v| v.2 >= cutoff);
            if self.healths.len() >= GOSSIP_HEALTH_CAP {
                // Pathologique (>200k santes <24 h) : garde la moitie
                // la plus recente.
                let mut aged: Vec<([u8; 20], u64)> =
                    self.healths.iter().map(|(k, v)| (*k, v.2)).collect();
                aged.sort_unstable_by_key(|(_, lc)| *lc);
                for (k, _) in aged.into_iter().take(GOSSIP_HEALTH_CAP / 2) {
                    self.healths.remove(&k);
                }
            }
        }
        self.healths
            .insert(h.infohash, (h.seeders, h.leechers, h.last_check))
            .is_none()
    }
}

/// `deprecated_parameters` Python : ces parametres de select sont
/// rejetes (reponse archive vide).
const DEPRECATED_SELECT_PARAMS: [&str; 3] = ["subscribed", "attribute_ranges", "complete_channel"];

impl SessionContentProvider {
    /// Convertit une ligne `channel_node` en entree `.mdblob`
    /// pre-signee (signature conservee telle quelle).
    fn row_to_entry(
        row: &onionbit_db::models::ChannelNodeRow,
    ) -> Option<onionbit_format::mdblob::MetadataEntry> {
        use onionbit_format::mdblob::*;
        let mut public_key = [0u8; 64];
        if row.public_key.len() != 64 {
            return None;
        }
        public_key.copy_from_slice(&row.public_key);
        let mut infohash = [0u8; 20];
        if row.infohash.len() != 20 {
            return None;
        }
        infohash.copy_from_slice(&row.infohash);
        let signature = row
            .signature
            .as_deref()
            .and_then(|s| <[u8; 64]>::try_from(s).ok())
            .unwrap_or([0u8; 64]);
        let node = ChannelNodePayload {
            header: SignedPayloadHeader::new(
                row.metadata_type as u16,
                row.reserved_flags as u16,
                public_key,
            )
            .with_signature(signature),
            id: row.id_ as u64,
            origin_id: row.origin_id as u64,
            timestamp: row.timestamp as u64,
        };
        let torrent = TorrentMetadataPayload {
            node,
            infohash,
            size: row.size.max(0) as u64,
            torrent_date: row.torrent_date.max(0) as u32,
            title: row.title.clone(),
            tags: row.tags.clone(),
            tracker_info: row.tracker_info.clone(),
        };
        Some(match row.metadata_type as u16 {
            types::CHANNEL_TORRENT => MetadataEntry::ChannelTorrent(ChannelMetadataPayload {
                torrent,
                num_entries: 0,
                start_timestamp: row.timestamp.max(0) as u64,
            }),
            _ => MetadataEntry::RegularTorrent(torrent),
        })
    }

    /// Corps du select distant (cf. `ContentProvider::remote_select`) —
    /// la requete SQL est deportee sur le pool bloquant (`db.call`).
    async fn remote_select_inner(&self, query: serde_json::Value) -> Vec<Vec<u8>> {
        // `sanitize_query` : `last` borne a `first + max_response_size`.
        let first = query.get("first").and_then(|v| v.as_u64()).unwrap_or(0);
        let last = query
            .get("last")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(first + self.max_response_size as u64))
            .unwrap_or(first + self.max_response_size as u64);

        let rows = self
            .db
            .call("content.remote_select", move |conn| {
                select_rows(conn, &query, first, last)
            })
            .await
            .unwrap_or_default();

        // `entries_to_chunk` Python : entrees serialisees groupees
        // par budget de `maximum_payload_size`, compressees LZ4 par
        // chunk — chaque chunk part dans un `SelectResponse` separe.
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let mut cur = Vec::new();
        for row in rows {
            let Some(entry) = Self::row_to_entry(&row) else {
                continue;
            };
            let Ok(bytes) = onionbit_format::mdblob::encode_entry_presigned(&entry) else {
                continue;
            };
            if cur.len() + bytes.len() > self.max_payload_size && !cur.is_empty() {
                chunks.push(lz4_frame(&cur));
                cur.clear();
            }
            cur.extend_from_slice(&bytes);
        }
        if !cur.is_empty() || chunks.is_empty() {
            chunks.push(lz4_frame(&cur));
        }
        chunks
    }
}

/// `lz4.frame.compress(b"")` Python (`LZ4_EMPTY_ARCHIVE`).
fn lz4_frame(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    if enc.write_all(data).is_err() {
        return Vec::new();
    }
    enc.finish().unwrap_or_default()
}

impl ContentProvider for SessionContentProvider {
    /// `get_random_torrents`/`get_popular_torrents` : santes jointes
    /// `channel_node`/`torrent_state`. Resultat mis en cache
    /// `healths_cache_ttl` par `request_type` — sous une rafale de
    /// `HEALTH_REQUEST` distants, une seule requete SQL est executee.
    fn healths_for<'a>(
        &'a self,
        request_type: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HealthInfo>> + Send + 'a>> {
        Box::pin(async move {
            if let Ok(cache) = self.healths_cache.lock() {
                if let Some((at, cached)) = cache.get(&request_type) {
                    if at.elapsed() < self.healths_cache_ttl {
                        return cached.clone();
                    }
                }
            }
            let popular = request_type == HEALTH_REQUEST_POPULAR;
            let fresh = self
                .db
                .call("content.healths_for", move |conn| {
                    let sql = if popular {
                        "SELECT n.infohash, t.seeders, t.leechers, t.last_check, n.tracker_info
                         FROM channel_node n JOIN torrent_state t ON t.rowid = n.health_rowid
                         WHERE n.infohash != '' ORDER BY t.seeders DESC LIMIT 20"
                    } else {
                        "SELECT n.infohash, t.seeders, t.leechers, t.last_check, n.tracker_info
                         FROM channel_node n JOIN torrent_state t ON t.rowid = n.health_rowid
                         WHERE n.infohash != '' ORDER BY RANDOM() LIMIT 20"
                    };
                    let mut stmt = conn.prepare(sql)?;
                    let rows = stmt.query_map([], |r| {
                        Ok((
                            r.get::<_, Vec<u8>>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, String>(4)?,
                        ))
                    })?;
                    Ok(rows
                        .filter_map(|r| r.ok())
                        .filter_map(|(ih, s, l, lc, tr)| {
                            let mut infohash = [0u8; 20];
                            if ih.len() != 20 {
                                return None;
                            }
                            infohash.copy_from_slice(&ih);
                            Some(HealthInfo {
                                infohash,
                                seeders: s.max(0) as u32,
                                leechers: l.max(0) as u32,
                                last_check: lc.max(0) as u64,
                                tracker: tr,
                            })
                        })
                        .collect::<Vec<_>>())
                })
                .await
                .unwrap_or_default();
            if let Ok(mut cache) = self.healths_cache.lock() {
                cache.insert(request_type, (std::time::Instant::now(), fresh.clone()));
            }
            fresh
        })
    }

    /// `process_torrents_health` : met a jour `torrent_state` ;
    /// retourne les infohashes inconnus (a resoudre par select).
    fn process_health<'a>(
        &'a self,
        healths: &'a [HealthInfo],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<[u8; 20]>> + Send + 'a>> {
        Box::pin(async move {
            // Santes recues gardees en memoire uniquement — plus de
            // persistance `torrent_state` : la table grossissait de
            // ~178k lignes d'historique de gossip et chaque rafale
            // monopolisait la connexion sqlite plusieurs secondes.
            // Les triplets (seeders/leechers/last_check) sont desormais
            // retenus en memoire (`GossipMemory`) : ils alimentent les
            // resultats distants et `TorrentHealthUpdated`.
            let mut updates = Vec::new();
            let unknown = {
                let Ok(mut g) = self.gossip.lock() else {
                    return Vec::new();
                };
                let mut unknown = Vec::new();
                for h in healths {
                    if g.insert_health(h) {
                        unknown.push(h.infohash);
                    }
                    if g.displayed.contains(&h.infohash) {
                        updates.push(h.clone());
                    }
                }
                unknown
            };
            // `torrent_health_updated` borne aux torrents affiches a
            // l'UI — le gossip brut produirait des centaines d'events
            // par tick pour des infohashes que personne ne voit.
            for h in updates {
                self.notifier
                    .notify(crate::notifier::Notification::TorrentHealthUpdated {
                        infohash: hex::encode(h.infohash),
                        seeders: h.seeders as i64,
                        leechers: h.leechers as i64,
                    });
            }
            unknown
        })
    }

    /// Select distant : parametres JSON (`txt_filter`, `infohash`,
    /// `infohash_set`, `first`/`last`, `metadata_type`, `channel_pk`,
    /// `origin_id`, `max_rowid`, `hide_xxx`) → chunks `.mdblob`
    /// compresses LZ4 (`send_db_results` Python : un `SelectResponse`
    /// par chunk, archive vide si rien).
    fn remote_select<'a>(
        &'a self,
        json: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            // `parse_parameters` Python : JSON invalide -> pas de
            // reponse exploitable (archive vide = `LZ4_EMPTY_ARCHIVE`).
            let Ok(query) = serde_json::from_slice::<serde_json::Value>(json) else {
                return vec![lz4_frame(&[])];
            };
            // `deprecated_parameters` : rejet (Python renvoie l'archive vide).
            if DEPRECATED_SELECT_PARAMS
                .iter()
                .any(|k| query.get(k).is_some())
            {
                tracing::warn!(%query, "remote select avec parametres deprecies");
                return vec![lz4_frame(&[])];
            }
            // `process_rpc_query_rate_limited` : une seule requete
            // `txt_filter` a la fois, les autres sont ignorees.
            let rate_limited = query.get("txt_filter").is_some();
            if rate_limited
                && self
                    .remote_queries_in_progress
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    > 0
            {
                self.remote_queries_in_progress
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                return vec![lz4_frame(&[])];
            }
            let result = self.remote_select_inner(query).await;
            if rate_limited {
                self.remote_queries_in_progress
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            result
        })
    }

    /// `process_compressed_mdblob` : decompresse LZ4, parse les
    /// entrees et retourne les `to_simple_dict()` des objets
    /// NOUVEAUX (`ObjState.NEW_OBJECT` — dedup `(public_key, id_)`
    /// en memoire). Les entrees ne sont PAS persistees dans
    /// `channel_node` : la table reste reservee a la reponse aux
    /// selects entrants (nos propres torrents).
    fn process_select_response<'a>(
        &'a self,
        blob: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<serde_json::Value>> + Send + 'a>>
    {
        Box::pin(async move {
            use std::io::Read;
            let mut data = Vec::new();
            if lz4_flex::frame::FrameDecoder::new(blob)
                .read_to_end(&mut data)
                .is_err()
            {
                return Vec::new();
            }
            let Ok(entries) = onionbit_format::mdblob::parse_blob(&data) else {
                return Vec::new();
            };
            // Les resultats ne sont plus persistes dans `channel_node`
            // (la table accumulait ~26k entrees de gossip et ses scans
            // figeaient la connexion sqlite partagee). Dedup en
            // memoire sur `(public_key, id_)` — meme semantique
            // `NEW_OBJECT` que `channel::insert`.
            let new_rows: Vec<onionbit_db::models::ChannelNodeRow> = {
                let Ok(mut seen) = self.seen_nodes.lock() else {
                    return Vec::new();
                };
                if seen.len() >= SEEN_NODES_CAP {
                    seen.clear();
                }
                entries
                    .iter()
                    .filter_map(|e| {
                        let row = entry_to_row(e)?;
                        seen.insert((row.public_key.clone(), row.id_))
                            .then_some(row)
                    })
                    .collect()
            };
            let (results, new_titles) = {
                let Ok(mut g) = self.gossip.lock() else {
                    return Vec::new();
                };
                let mut results = Vec::new();
                let mut new_titles = Vec::new();
                for row in &new_rows {
                    let ih: Option<[u8; 20]> = row.infohash.as_slice().try_into().ok();
                    if let Some(ih) = ih {
                        if g.displayed.len() < DISPLAYED_CAP {
                            g.displayed.insert(ih);
                        }
                    }
                    results.push(simple_dict_mem(
                        row,
                        ih.and_then(|k| g.healths.get(&k).copied()),
                    ));
                    if !row.title.is_empty() {
                        new_titles.push((hex::encode(&row.infohash), row.title.clone()));
                    }
                }
                (results, new_titles)
            };
            // `torrent_metadata_added` : le notifier consomme aussi les
            // titres pour l'apprentissage de l'augmenteur.
            for (infohash, title) in new_titles {
                self.notifier
                    .notify(crate::notifier::Notification::TorrentMetadataCreated {
                        infohash,
                        title,
                    });
            }
            results
        })
    }

    /// Derniere sante gossip connue pour `infohash` — repli memoire de
    /// `/api/metadata/torrents/{ih}/health` quand `torrent_state` n'a
    /// pas de ligne (les resultats distants n'y figurent plus).
    fn known_health(&self, infohash: &[u8; 20]) -> Option<HealthInfo> {
        let (seeders, leechers, last_check) = *self.gossip.lock().ok()?.healths.get(infohash)?;
        Some(HealthInfo {
            infohash: *infohash,
            seeders,
            leechers,
            last_check,
            tracker: String::new(),
        })
    }

    /// `(version, plateforme)` pour `VersionResponse` —
    /// `f"Tribler {version}"` + `sys.platform` Python.
    fn version_info(&self) -> (String, String) {
        let platform = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        (
            format!("Tribler {}", env!("CARGO_PKG_VERSION")),
            platform.into(),
        )
    }
}

/// Convertit une entree `.mdblob` en ligne `channel_node` (insertion).
fn entry_to_row(
    entry: &onionbit_format::mdblob::MetadataEntry,
) -> Option<onionbit_db::models::ChannelNodeRow> {
    use onionbit_format::mdblob::*;
    let (node, torrent) = match entry {
        MetadataEntry::RegularTorrent(t) => (&t.node, Some(t)),
        MetadataEntry::ChannelTorrent(c) => (&c.torrent.node, Some(&c.torrent)),
        _ => return None,
    };
    let t = torrent?;
    Some(onionbit_db::models::ChannelNodeRow {
        rowid: 0,
        infohash: t.infohash.to_vec(),
        size: t.size as i64,
        torrent_date: t.torrent_date as i64,
        tracker_info: t.tracker_info.clone(),
        title: t.title.clone(),
        tags: t.tags.clone(),
        metadata_type: node.header.metadata_type as i64,
        reserved_flags: node.header.reserved_flags as i64,
        origin_id: node.origin_id as i64,
        public_key: node.header.public_key.to_vec(),
        id_: node.id as i64,
        timestamp: node.timestamp as i64,
        signature: Some(node.header.signature.to_vec()),
        added_on: now_unix() as i64,
        status: 1,
        xxx: 0.0,
        health_rowid: None,
        tag_processor_version: 0,
        health_seeders: None,
        health_leechers: None,
        health_last_check: None,
    })
}

/// `TorrentMetadata.to_simple_dict()` Python pour les resultats de
/// recherche distants : meme forme JSON que `remote_query_results`.
/// `health` = derniere sante gossip connue (memoire — l'equivalent du
/// `torrent_state` Python joint par `to_simple_dict`, sans persistance) ;
/// `None` -> 0/0/0 comme Python pour une entree sans sante connue.
fn simple_dict_mem(
    row: &onionbit_db::models::ChannelNodeRow,
    health: Option<(u32, u32, u64)>,
) -> serde_json::Value {
    let (seeders, leechers, last_check) = health.unwrap_or((0, 0, 0));
    serde_json::json!({
        "name": row.title,
        "category": row.tags,
        "infohash": hex::encode(&row.infohash),
        "size": row.size,
        "num_seeders": seeders,
        "num_leechers": leechers,
        "last_tracker_check": last_check,
        "created": row.torrent_date,
        "updated": row.timestamp,
        "tag_processor_version": row.tag_processor_version,
        "type": row.metadata_type,
        "id": row.id_,
        "origin_id": row.origin_id,
        "public_key": hex::encode(&row.public_key),
        "status": row.status,
        "xxx": row.xxx,
        "trackers": [],
    })
}

/// Secondes Unix courantes.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Champ binaire hex d'une requete select (`binary_fields` Python :
/// `infohash`, `channel_pk` arrives en hex).
fn json_hex_bytes(query: &serde_json::Value, key: &str) -> Option<Vec<u8>> {
    query
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| hex::decode(s.trim_matches('"')).ok())
}

/// `metadata_type` Python : accepte entier, chaine ou liste
/// (`convert_to_json` convertit la chaine en `[int]`).
fn json_metadata_types(query: &serde_json::Value) -> Option<Vec<i64>> {
    let v = query.get("metadata_type")?;
    let one = |v: &serde_json::Value| -> Option<i64> {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    match v {
        serde_json::Value::Array(arr) => Some(arr.iter().filter_map(one).collect()),
        other => one(other).map(|m| vec![m]),
    }
}

/// Filtre les lignes `channel_node` selon la requete JSON distante
/// (`get_entries` Python : `txt_filter` FTS -> AND de LIKE,
/// `infohash`/`infohash_set`, `channel_pk`, `origin_id`,
/// `metadata_type`, `max_rowid`, `hide_xxx`), puis tranche
/// `first`..`last` de `sanitize_query`.
fn select_rows(
    conn: &rusqlite::Connection,
    query: &serde_json::Value,
    first: u64,
    last: u64,
) -> onionbit_db::Result<Vec<onionbit_db::models::ChannelNodeRow>> {
    // `txt_filter` arrive deja formate FTS (`"a" "b"`) : il part en
    // `FtsIndex MATCH` tel quel ; `terms` sert de repli `LIKE`.
    let txt = query
        .get("txt_filter")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let params = onionbit_db::channel::SelectParams {
        txt_filter: txt.clone(),
        terms: txt
            .as_deref()
            .map(crate::queries::fts_terms)
            .unwrap_or_default(),
        metadata_types: json_metadata_types(query),
        infohash: json_hex_bytes(query, "infohash"),
        infohash_set: query
            .get("infohash_set")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| hex::decode(s.trim_matches('"')).ok())
                    .collect()
            })
            .unwrap_or_default(),
        channel_pk: json_hex_bytes(query, "channel_pk"),
        origin_id: query.get("origin_id").and_then(|v| v.as_i64()),
        id_: query.get("id").and_then(|v| v.as_i64()),
        max_rowid: query.get("max_rowid").and_then(|v| v.as_i64()),
        hide_xxx: query
            .get("hide_xxx")
            .and_then(|v| {
                v.as_bool()
                    .or_else(|| v.as_str().map(|s| s == "1" || s == "true"))
            })
            .unwrap_or(false),
        sort_by: query
            .get("sort_by")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        sort_desc: query
            .get("sort_desc")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        category: query
            .get("category")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tags: query
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        first,
        last: Some(last),
        ..Default::default()
    };
    onionbit_db::channel::select_entries(conn, &params)
}

/// Stack IPv8 de session (endpoint + communities + lanes anonymes).
pub struct Ipv8Stack {
    /// Endpoint UDP IPv8 partage.
    pub endpoint: Arc<UdpEndpoint>,
    /// Registre de pairs IPv8.
    pub network: Arc<Network>,
    /// Community de decouverte de pairs.
    pub discovery: Arc<DiscoveryCommunity>,
    /// Community de decouverte de contenu (`None` si
    /// `content_discovery_community/enabled = false`).
    pub content_discovery: Option<Arc<ContentDiscoveryCommunity>>,
    /// `TunnelCommunity` (presente si `enable_anonymity`).
    pub tunnel: Option<Arc<TunnelCommunity>>,
    /// `DHTDiscoveryCommunity` (presente si `enable_dht` —
    /// `dht_discovery/enabled` Python). Sert aussi de `DHTCommunity`
    /// pour les routes `/api/ipv8/dht/*`.
    pub dht: Option<Arc<DhtCommunity>>,
    /// `OnionbitExtCommunity` (ADR-0015, presente si `ext_enabled`) —
    /// communaute OnionBit-only, `hello` lazy vers pairs deja connus.
    pub ext: Option<Arc<onionbit_ipv8::ext::OnionbitExtCommunity>>,
    /// Cle IPv8 de la session (persistee dans `state_dir`).
    key: LibNaClSecretKey,
    /// Arret de la tache de maintenance DHT.
    dht_maintenance_stop: Option<tokio::sync::watch::Sender<bool>>,
    /// Moteurs anonymes par nombre de sauts (1..=3).
    anon_lanes: Mutex<HashMap<usize, AnonLane>>,
    /// Serialise le get-or-create de `anon_engine` : deux adds
    /// concurrents sur une lane neuve creaient chacun un moteur, le
    /// second insert ecrasait le premier — le download ajoute au
    /// moteur perdant devenait orphelin (invisible de
    /// `owner_engine_hops`/`downloads()`/restauration).
    anon_engine_lock: tokio::sync::Mutex<()>,
    /// Config moteur de base (pour creer les lanes anonymes).
    engine_config: EngineConfig,
    /// Repertoire de telechargement par defaut.
    downloads_dir: PathBuf,
    /// `state_dir` (dossiers de persistance fastresume des lanes).
    state_dir: PathBuf,
    /// Registre partage des taches nommees (`/api/ipv8/asyncio/tasks`).
    tasks: crate::asyncio::TaskRegistry,
    /// Bornes `min_circuits`/`max_circuits` partagees avec les
    /// watchdogs de lanes : `POST /api/settings` les met a jour a
    /// chaud (`set_circuit_bounds`) — Python relit `self.settings`
    /// a chaque tick de `monitor_downloads`.
    circuit_bounds: Arc<(AtomicUsize, AtomicUsize)>,
    /// `libtorrent/socks_listen_ports` (ports SOCKS5 par lane).
    socks_listen_ports: Vec<u16>,
    /// Discipline DHT des lanes anonymes (extensions Rust —
    /// `Ipv8Config::anon_dht_*`) : plafond de debit socket, posture
    /// client-only et plafond de backoff `get_peers`.
    anon_dht_rate_pps: u64,
    /// `anon_dht_client_only` (voir `Ipv8Config`).
    anon_dht_client_only: bool,
    /// `anon_dht_backoff_cap_secs` (voir `Ipv8Config`).
    anon_dht_backoff_cap_secs: u64,
    /// `monitor_hidden_swarms` Python : dernier etat connu par
    /// `(hops, lookup_info_hash)` pour detecter les transitions
    /// join/leave des swarms caches.
    swarm_states: Mutex<HashMap<(usize, [u8; 20]), onionbit_bittorrent::DownloadState>>,
    /// `lookup_info_hash -> (hops, info_hash reel)` : les circuits
    /// e2e notifies par `e2e_ready` portent l'info-hash de LOOKUP du
    /// swarm, ce mapping retrouve le download correspondant.
    swarm_lookup: Mutex<SwarmLookupMap>,
    /// Sinks de pairs des magnets encore en resolution (`pending`),
    /// `info_hash reel -> sender` : `spawn_e2e_listener` y pousse
    /// l'adresse factice quand `get_by_hash` ne trouve pas encore le
    /// torrent — sinon le pair e2e etait perdu et la resolution
    /// magnet restait bloquee sans jamais voir le seeder cache.
    pending_peer_sinks: Mutex<HashMap<[u8; 20], tokio::sync::mpsc::UnboundedSender<SocketAddr>>>,
    /// Service messagerie e2e (ADR-0011) — present si
    /// `enable_messaging` et `enable_anonymity` : joints le swarm
    /// `messaging_hash(pk)` et demultiplexe les cellules `data` de
    /// ses circuits e2e (separe de la lane uTP — `spawn_e2e_listener`
    /// ignore les swarms hors `swarm_lookup`).
    pub messaging: Option<Arc<crate::services::messaging::MessagingService>>,
    /// Arrets des taches hidden-services (monitor des swarms +
    /// relais `e2e_ready`), creees a la premiere lane anonyme.
    hidden_tasks: Mutex<Vec<tokio::sync::watch::Sender<bool>>>,
    /// `Weak` auto-reference (les taches hidden-services demarrees
    /// depuis `anon_engine(&self)` n'ont pas acces a `Arc<Self>`).
    self_weak: Mutex<std::sync::Weak<Ipv8Stack>>,
}

/// Fichier de persistance des noeuds de sortie (`exitnode_cache`
/// Python) dans `state_dir` : lignes `<pubkey_hex> <addr> <flags>`.
const EXITNODE_CACHE_FILE: &str = "exitnodes.txt";

/// Cadence de `monitor_hidden_swarms` Python (poll des etats de
/// telechargement via `DownloadManager` / `get_last_download_states`).
const SWARM_MONITOR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Mapping `lookup_info_hash -> (hops, info_hash reel)` des swarms
/// caches (champ `swarm_lookup` de `Ipv8Stack`).
type SwarmLookupMap = HashMap<[u8; 20], (usize, [u8; 20])>;

/// Liaison download anonyme <-> swarm cache, exposee pour
/// `GET /api/ipv8/tunnel/debug/circuit-downloads` (extension Rust —
/// Python garde `download_states` interne sans endpoint dedie).
#[derive(Debug, Clone)]
pub struct SwarmDownload {
    /// Info-hash de lookup du swarm — aussi `info_hash` porte par les
    /// circuits `IP_*`/`RP_*` du swarm (cle de jointure).
    pub lookup_info_hash: [u8; 20],
    /// Info-hash reel du torrent (`infohash` du download).
    pub info_hash: [u8; 20],
    /// Lane de sauts du download (`anon_hops`).
    pub hops: usize,
    /// Dernier etat vu par `monitor_hidden_swarms`.
    pub state: onionbit_bittorrent::DownloadState,
}

impl Ipv8Stack {
    /// Cree et demarre la stack : endpoint, communities, discovery
    /// bootstrap (tache de fond). `notifier` recoit le relais
    /// `circuit_removed` -> `tunnel_removed`.
    pub async fn start(
        config: &Ipv8Config,
        state_dir: &Path,
        downloads_dir: &Path,
        engine_config: &EngineConfig,
        db: Arc<Database>,
        notifier: crate::notifier::Notifier,
        tasks: crate::asyncio::TaskRegistry,
    ) -> Result<Arc<Self>> {
        let key = load_or_create_key(&state_dir.join(IPV8_KEY_FILE))?;
        // `ipv8/interfaces` pyipv8 : `UDPIPv4` obligatoire, `UDPIPv6`
        // optionnel (socket secondaire du meme endpoint, partage des
        // listeners — `DispatcherEndpoint`).
        let bind_v6 = config.listen_addr_v6.as_deref();
        let endpoint = UdpEndpoint::bind_dual_with_retry(
            &config.listen_addr,
            bind_v6,
            onionbit_ipv8::endpoint::UdpEndpoint::MAX_PORT_RETRY_ATTEMPTS,
        )
        .await
        .map_err(|e| CoreError::State(format!("bind ipv8: {e}")))?;
        let endpoint: Arc<UdpEndpoint> = endpoint;
        let network = Arc::new(Network::default());
        // `my_estimated_lan` : l'adresse d'ecoute reelle quand elle
        // est specifiee (127.0.0.1 du banc, NIC LAN en prod) — pyipv8
        // la resout via `get_lan_addresses()`. Declarer `0.0.0.0`
        // faisait wrapper `peer.address = (0.0.0.0, port)` chez les
        // pairs pyipv8 : le `destination_address` renvoye restait
        // non specifie et la DHT s'affamait meme face a Tribler.
        let lan = {
            let local = endpoint
                .local_addr()
                .map_err(|e| CoreError::State(format!("addr ipv8: {e}")))?;
            let lan_ip = match local {
                std::net::SocketAddr::V4(a) if !a.ip().is_unspecified() => *a.ip(),
                _ => std::net::Ipv4Addr::UNSPECIFIED,
            };
            UdpAddress::Ipv4(std::net::SocketAddrV4::new(lan_ip, local.port()))
        };
        let discovery =
            DiscoveryCommunity::new(key.clone(), network.clone(), endpoint.clone(), lan.clone())
                .await;
        // Extension Rust (bancs loopback) : `ipv8.estimated_wan` —
        // sur un mesh 100 % loopback `my_estimated_wan` ne s'apprend
        // jamais (`address_in_lan_subnets`), ce qui figerait le DHT
        // (`on_node_discovered` refuse tout noeud — comme pyipv8).
        if let Some(wan) = &config.estimated_wan {
            discovery.set_estimated_wan(wan.clone());
        }
        // `ContentDiscoveryComponent` Python : cree seulement si
        // `content_discovery_community/enabled` (la recherche distante
        // et `/api/search` retournent alors 503-vide cote REST).
        let content_discovery = if config.enable_content_discovery {
            // `ContentDiscoverySettings` Python : defauts filaires
            // (gossip 5 s, max 20 pairs, TTL 10 s, 10 paquets max).
            let cd_settings = onionbit_ipv8::content_discovery::ContentDiscoverySettings::default();
            let provider = Arc::new(SessionContentProvider {
                db: db.clone(),
                notifier: notifier.clone(),
                remote_queries_in_progress: std::sync::atomic::AtomicUsize::new(0),
                max_response_size: 100,
                max_payload_size: 1300,
                healths_cache: Mutex::new(HashMap::new()),
                healths_cache_ttl: std::time::Duration::from_secs(
                    config.content_healths_cache_secs,
                ),
                seen_nodes: Mutex::new(std::collections::HashSet::new()),
                gossip: Mutex::new(GossipMemory::default()),
            });
            Some(
                ContentDiscoveryCommunity::new(
                    key.clone(),
                    network.clone(),
                    endpoint.clone(),
                    provider,
                    cd_settings,
                    discovery.clone(),
                )
                .await,
            )
        } else {
            None
        };
        // ADR-0015 §1–2 : `OnionbitExtCommunity` — extension
        // OnionBit-only sur `community_id` dedie. `hello` lazy vers
        // les pairs deja verifies (aucune marche : la liste de
        // bootstrap n'est jamais sondee sur ce prefixe) ; le peer set
        // de l'overlay *est* la population OnionBit visible.
        let ext = if config.ext_enabled {
            let e = onionbit_ipv8::ext::OnionbitExtCommunity::new(
                key.clone(),
                network.clone(),
                endpoint.clone(),
                onionbit_ipv8::ext::ExtSettings {
                    hello_interval: std::time::Duration::from_secs(
                        config.ext_hello_interval_secs.max(1),
                    ),
                    hello_fanout: config.ext_hello_fanout as usize,
                    hello_cooldown: std::time::Duration::from_secs(
                        config.ext_hello_cooldown_secs.max(1),
                    ),
                    curators: config.ext_curators.iter().cloned().collect(),
                    attest_max_future_skew: std::time::Duration::from_secs(
                        config.ext_attest_max_future_skew_secs.max(1),
                    ),
                    attest_list_max: config.ext_attest_list_max as usize,
                    attest_rate_window: std::time::Duration::from_secs(
                        config.ext_attest_rate_window_secs.max(1),
                    ),
                    attest_rate_max: config.ext_attest_rate_max,
                    attest_rate_table_max: config.ext_attest_rate_table_max as usize,
                    ledger_enabled: config.ext_ledger_enabled,
                    ledger_tranche_bytes: config.ext_ledger_tranche_bytes.max(1),
                    ledger_settle_interval: std::time::Duration::from_secs(
                        config.ext_ledger_settle_interval_secs.max(1),
                    ),
                    ledger_propose_timeout: std::time::Duration::from_secs(
                        config.ext_ledger_propose_timeout_secs.max(1),
                    ),
                    ledger_max_retries: config.ext_ledger_max_retries,
                    ledger_drift_permille: config.ext_ledger_drift_permille,
                    ledger_drift_min: config.ext_ledger_drift_min_bytes,
                    ledger_rate_window: std::time::Duration::from_secs(
                        config.ext_ledger_rate_window_secs.max(1),
                    ),
                    ledger_rate_max: config.ext_ledger_rate_max,
                    ledger_rate_table_max: config.ext_ledger_rate_table_max as usize,
                    ledger_store_max: config.ext_ledger_store_max as usize,
                    ledger_head_fanout: config.ext_ledger_head_fanout as usize,
                    obf_enabled: config.ext_obf_enabled,
                    obf_pad_bucket: config.ext_obf_pad_bucket.max(16) as usize,
                    // `CAP_MSG_V1` : annonce seulement quand le
                    // service messagerie ADR-0011 demarre
                    // reellement — il faut le tunnel (`enable_anonymity`)
                    // pour porter les lanes e2e.
                    messaging_enabled: config.enable_messaging && config.enable_anonymity,
                    hello_jitter_pct: config.ext_hello_jitter_pct.min(100) as u8,
                    ..onionbit_ipv8::ext::ExtSettings::default()
                },
            )
            .await;
            e.set_attestation_store(Arc::new(crate::attestation_store::DbAttestationStore::new(
                db.clone(),
            )));
            // Phase 9c : persistance des liens du ledger bilateral
            // dans `ext_ledger_links` (v18).
            e.set_ledger_store(Arc::new(crate::ext_ledger_store::DbLedgerStore::new(
                db.clone(),
            )));
            tasks.register(Some("OnionbitExtCommunity"), "hello", None);
            let e2 = e.clone();
            tokio::spawn(async move {
                e2.run().await;
            });
            Some(e)
        } else {
            None
        };
        let dht = if config.enable_dht {
            // `DHTDiscoveryCommunity` Python : `my_estimated_wan`
            // commence non-specifie et est appris par introduction ;
            // `my_estimated_lan` = adresse d'ecoute (ci-dessus).
            Some(
                DhtCommunity::new(
                    key.clone(),
                    UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::UNSPECIFIED,
                        0,
                    )),
                    lan,
                    endpoint.clone(),
                )
                .await,
            )
        } else {
            None
        };
        let tunnel = if config.enable_anonymity {
            let community_id = if config.onionbit_tunnel_community {
                TRIBLER_TUNNEL_COMMUNITY_ID
            } else {
                onionbit_tunnel::TUNNEL_COMMUNITY_ID
            };
            let t = TunnelCommunity::new_with_id(
                key.clone(),
                network.clone(),
                endpoint.clone(),
                onionbit_tunnel::settings::TunnelSettings {
                    peer_flags: config.peer_flags,
                    min_circuits: config.min_circuits.max(1) as usize,
                    max_circuits: config.max_circuits.max(1) as usize,
                    max_joined_circuits: config.max_joined_circuits,
                    // Mode auto (-1) : plafond provisoire
                    // `bandwidth.fallback_bps` jusqu'a la mesure.
                    max_relayed_bps: initial_relay_bps(config.max_relayed_bps, &config.bandwidth),
                    intro_point_peer: config.intro_point_peer.clone(),
                    data_exit_peer: config.data_exit_peer.clone(),
                    guards: onionbit_tunnel::guards::GuardsConfig {
                        enabled: config.guards_enabled,
                        ..onionbit_tunnel::guards::GuardsConfig::default()
                    },
                    exit_inbound_source_ttl: std::time::Duration::from_secs(
                        config.exit_inbound_source_ttl_secs,
                    ),
                    exit_inbound_max_sources: config.exit_inbound_max_sources,
                    // ADR-0015 : comptabilite locale par pair —
                    // `soft_cap` borne par `max_joined_circuits` (la
                    // gate ne doit jamais empecher `max_joined`
                    // d'etre la limite effective).
                    ledger: onionbit_tunnel::peer_stats::LedgerConfig {
                        enabled: config.ledger_enabled,
                        enforce: config.ledger_enforce,
                        soft_cap: config
                            .ledger_soft_cap
                            .min(config.max_joined_circuits.saturating_sub(1)),
                        max_deficit_bytes: config.ledger_max_deficit_bytes,
                        tick: std::time::Duration::from_secs(config.ledger_tick_secs.max(1)),
                        max_peers: config.ledger_max_peers,
                    },
                    ..onionbit_tunnel::settings::TunnelSettings::default()
                },
                community_id,
            )
            .await;
            // ADR-0010 : persistance des guards dans `onionbit.db` —
            // injectee seulement quand la feature est activee (set
            // volatile sinon, selection pyipv8 exacte).
            if config.guards_enabled {
                t.set_guard_store(Arc::new(crate::guard_store::DbGuardStore::new(db.clone())));
            }
            // ADR-0015 : persistance du ledger par pair dans
            // `peer_stats` — injectee des que la collecte est
            // activee (le store recharge les comptes au boot).
            if config.ledger_enabled {
                t.set_peer_stats_store(Arc::new(crate::peer_stats_store::DbPeerStatsStore::new(
                    db.clone(),
                )));
            }
            // `my_peer` Python est partage entre overlays : la
            // tunnel-community emprunte les estimations WAN/LAN de la
            // discovery pour ses introductions et punctures.
            t.set_discovery(discovery.clone());
            // `dht_provider` (`out["dht_provider"] =
            // DHTCommunityProvider(...)` du composant Tribler) :
            // annonces/lookups des points d'introduction des swarms
            // caches dans la `DHTDiscoveryCommunity`.
            if let Some(d) = &dht {
                t.set_dht_provider(d.clone());
            }
            // `register_task("do_circuits")`/`do_ping`/
            // `do_peer_discovery` Python : maintenance des circuits
            // (nettoyage, keepalive, lookups de swarms caches).
            tasks.register(Some("TriblerTunnelCommunity"), "maintenance", None);
            let t_maint = t.clone();
            tokio::spawn(async move {
                t_maint.run_maintenance().await;
            });
            // `load_exit_nodes` Python : reintroduit les noeuds de
            // sortie connus de la session precedente dans `Network`
            // puis les re-sollicite (`send_introduction_request`).
            for (pk, addr, flags) in load_exitnode_cache(&state_dir.join(EXITNODE_CACHE_FILE)) {
                t.register_exit_peer(&pk, addr, flags);
                let t = t.clone();
                tokio::spawn(async move {
                    let _ = t.send_introduction_request(&UdpAddress::from(addr)).await;
                });
            }
            Some(t)
        } else {
            None
        };

        // ADR-0015 §5 (Phase 9c) : boucle entre le ledger bilateral
        // ext et la comptabilite locale `peer_stats` (9a) —
        // `stats_source` alimente les propositions/gates de la
        // comptabilite mesuree, `sign_veto` fait respecter
        // sign-then-serve dans `admit` quand `ledger_enforce` est on.
        if let (Some(e), Some(t)) = (&ext, &tunnel) {
            let t2 = t.clone();
            e.set_stats_source(Arc::new(move |pk| {
                t2.ledger
                    .stat(pk)
                    .map(|st| (st.bytes_served, st.bytes_used))
            }));
            let e2 = e.clone();
            t.ledger
                .set_sign_veto(Arc::new(move |pk| e2.owes_signature(pk)));
        }

        // Stores PEX persistes (`tunnel_pex`) : on redevient point
        // d'introduction des swarms connus des le demarrage — le
        // hidden seeding reste joignable sans attendre un nouveau
        // `establish-intro` du seeder.
        if let Some(t) = &tunnel {
            let pex_rows = db
                .call("ipv8.pex_list", onionbit_db::pex::list)
                .await
                .unwrap_or_default();
            if !pex_rows.is_empty() {
                type Grouped = HashMap<
                    [u8; 20],
                    (
                        Vec<onionbit_tunnel::routing::IntroductionPoint>,
                        Vec<Vec<u8>>,
                    ),
                >;
                let mut grouped = Grouped::new();
                for r in pex_rows {
                    let Ok(ih) = <[u8; 20]>::try_from(r.info_hash.as_slice()) else {
                        continue;
                    };
                    let entry = grouped.entry(ih).or_default();
                    if r.own {
                        entry.1.push(r.seeder_pk);
                    } else if let Ok(sa) = r.address.parse::<SocketAddr>() {
                        entry.0.push(onionbit_tunnel::routing::IntroductionPoint {
                            address: UdpAddress::from(sa),
                            peer_key: r.peer_key,
                            seeder_pk: r.seeder_pk,
                            source: r.source as u8,
                            last_seen_secs: r.last_seen as u64,
                        });
                    }
                }
                let n = grouped.len();
                t.pex_restore(
                    grouped
                        .into_iter()
                        .map(|(ih, (learned, announces))| (ih, learned, announces))
                        .collect(),
                );
                tracing::info!(swarms = n, "stores PEX restaures depuis la base");
            }
        }

        // Relais `circuit_removed` -> `Notification::TunnelRemoved`
        // (onionbit-tunnel ne depend pas de onionbit-core : pont par
        // canal broadcast, equivalent du Notifier partage Python).
        if let Some(t) = &tunnel {
            let mut rx = t.watch_circuit_removed();
            let notifier = notifier.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => notifier.notify(crate::notifier::Notification::TunnelRemoved {
                            circuit_id: ev.circuit_id,
                            circuit_class: ev.circuit_class.to_string(),
                            bytes_up: ev.bytes_up,
                            bytes_down: ev.bytes_down,
                            uptime_secs: ev.uptime_secs,
                            additional_info: ev.additional_info,
                        }),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
            });
        }

        // `enable_overlay_statistics(enable=True, all_overlays=True)`
        // de `session.py` : Tribler active le suivi des stats pour
        // toutes les communities des le demarrage.
        let stats_prefixes = {
            let mut v = vec![onionbit_ipv8::prefix_of(
                &onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID,
            )];
            if content_discovery.is_some() {
                v.push(onionbit_ipv8::prefix_of(
                    &onionbit_ipv8::CONTENT_DISCOVERY_COMMUNITY_ID,
                ));
            }
            if let Some(t) = &tunnel {
                v.push(onionbit_ipv8::prefix_of(&t.community_id()));
            }
            if dht.is_some() {
                v.push(onionbit_ipv8::prefix_of(
                    &onionbit_ipv8::dht::DHT_COMMUNITY_ID,
                ));
            }
            v
        };
        for prefix in stats_prefixes {
            endpoint.enable_community_statistics(prefix, true).await;
        }

        // Tache de reception de l'endpoint (dispatch prefixe) —
        // `ensure_future(endpoint.run())` Python (tache anonyme).
        tasks.register(None, "endpoint", None);
        let ep = endpoint.clone();
        tokio::spawn(async move {
            if let Err(e) = ep.run().await {
                tracing::error!(error = %e, "endpoint ipv8 termine");
            }
        });

        // Echantillonnage des compteurs de l'endpoint pour le debit
        // `rate_up`/`rate_down` de `/api/statistics/ipv8` (extension
        // Rust) : cadence interne au daemon — aucune mesure a faire
        // cote client, toutes les UI affichent la meme valeur.
        tasks.register(None, "endpoint_rates", None);
        let ep = endpoint.clone();
        let rate_interval = std::time::Duration::from_millis(config.stats_rate_sample_ms);
        let rate_span = std::time::Duration::from_secs(config.stats_rate_window_secs);
        tokio::spawn(async move {
            ep.run_rate_sampler(rate_interval, rate_span).await;
        });

        // Maintenance DHT en tache de fond (arret via `stop`).
        let dht_maintenance_stop = dht
            .clone()
            .map(|d| spawn_dht_maintenance(d, discovery.clone(), tasks.clone()));

        // Cache de pairs verifies (`ipv8_peers`) : recharge les plus
        // frais dans `Network` AVANT le bootstrap — les walks peuvent
        // viser des pairs connus immediatement, sans attendre la
        // resolution DNS ni un premier cycle d'introduction.
        let peer_cutoff = now_unix().saturating_sub(config.peer_cache_max_age_secs) as i64;
        let peer_cache_max = config.peer_cache_max;
        let restored = db
            .call("ipv8.peers_restore", move |c| {
                onionbit_db::peers::list_peers(c, peer_cutoff, peer_cache_max)
            })
            .await
            .unwrap_or_default();
        let mut warm_addrs: Vec<UdpAddress> = Vec::new();
        for row in &restored {
            if let Ok(sa) = row.address.parse::<SocketAddr>() {
                let addr = UdpAddress::from(sa);
                let mut peer = match Peer::new(row.public_key.clone(), Some(addr.clone())) {
                    Some(p) => p,
                    None => continue,
                };
                peer.new_style_intro = row.new_style;
                network.add_verified(peer);
                warm_addrs.push(addr);
            }
        }
        if !warm_addrs.is_empty() {
            tracing::info!(
                restored = warm_addrs.len(),
                "pairs IPv8 restaures depuis le cache persistant"
            );
        }

        // Snapshot periodique `Network` -> `ipv8_peers` : une ecriture
        // en lot par intervalle plutot qu'un upsert par pair decouvert
        // (le WAL + `call` gardent la boucle d'evenements fluide).
        tasks.register(
            None,
            "ipv8_peer_cache",
            Some(config.peer_persist_interval_secs as f64),
        );
        {
            let network = network.clone();
            let db = db.clone();
            let tunnel_cache = tunnel.clone();
            let interval = std::time::Duration::from_secs(config.peer_persist_interval_secs.max(5));
            let max = config.peer_cache_max;
            let max_age = config.peer_cache_max_age_secs;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let now = now_unix() as i64;
                    let cutoff = now.saturating_sub(max_age as i64);
                    // Stores PEX : annonces propres + points appris
                    // (meme snapshot que `ipv8_peers`, meme ecriture).
                    let pex_rows: Vec<onionbit_db::PexRow> = tunnel_cache
                        .as_ref()
                        .map(|t| {
                            t.pex_dump()
                                .into_iter()
                                .flat_map(|(ih, learned, announces)| {
                                    let mut v = Vec::with_capacity(learned.len() + announces.len());
                                    for ip in learned {
                                        v.push(onionbit_db::PexRow {
                                            info_hash: ih.to_vec(),
                                            own: false,
                                            peer_key: ip.peer_key,
                                            seeder_pk: ip.seeder_pk,
                                            address: ip
                                                .address
                                                .to_socket_addr()
                                                .map(|s| s.to_string())
                                                .unwrap_or_default(),
                                            source: ip.source as i64,
                                            last_seen: ip.last_seen_secs as i64,
                                        });
                                    }
                                    for pk in announces {
                                        v.push(onionbit_db::PexRow {
                                            info_hash: ih.to_vec(),
                                            own: true,
                                            peer_key: Vec::new(),
                                            seeder_pk: pk,
                                            address: String::new(),
                                            source: 0,
                                            last_seen: 0,
                                        });
                                    }
                                    v
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let write_pex = tunnel_cache.is_some();
                    let rows: Vec<onionbit_db::Ipv8PeerRow> = network
                        .verified_peers()
                        .iter()
                        .filter_map(|p| {
                            let sa = p.address.as_ref()?.to_socket_addr()?;
                            if sa.ip().is_unspecified() {
                                return None;
                            }
                            Some(onionbit_db::Ipv8PeerRow {
                                public_key: p.public_key_bin.clone(),
                                address: sa.to_string(),
                                last_seen: now,
                                new_style: p.new_style_intro,
                            })
                        })
                        .collect();
                    let _ = db
                        .call("ipv8.peers_pex_persist", move |c| {
                            if !rows.is_empty() {
                                onionbit_db::peers::upsert_batch(c, &rows, now)?;
                                onionbit_db::peers::prune(c, cutoff, max)?;
                            }
                            if write_pex {
                                onionbit_db::pex::replace_all(c, &pex_rows)?;
                            }
                            Ok(())
                        })
                        .await;
                }
            });
        }

        // Bootstrap discovery en tache de fond (resolution DNS + IP).
        // `register_anonymous_task("bootstrap", ...)` Python. Les
        // adresses du cache sont marchees en plus des noeuds
        // d'amorcage (le cache seul suffit a amorcer).
        let bootstrap_peers_config = config.bootstrap_peers.clone();
        if !bootstrap_peers_config.is_empty() || !warm_addrs.is_empty() {
            tasks.register(None, "bootstrap", None);
            let d = discovery.clone();
            let dht = dht.clone();
            let cd = content_discovery.clone();
            let tunnel_for_walk = tunnel.clone();
            let walker_interval = std::time::Duration::from_secs_f64(config.walker_interval);
            tokio::spawn(async move {
                let mut peers = Vec::new();
                for peer_str in &bootstrap_peers_config {
                    if let Ok(sa) = peer_str.parse::<SocketAddr>() {
                        peers.push(UdpAddress::from(sa));
                    } else if let Ok(mut resolved) =
                        tokio::net::lookup_host(peer_str.as_str()).await
                    {
                        if let Some(sa) = resolved.next() {
                            peers.push(UdpAddress::from(sa));
                        }
                    }
                }
                // Pairs caches : ajoutes aux cibles de marche (dedup
                // — un noeud d'amorcage peut aussi etre un pair connu).
                for a in warm_addrs {
                    if !peers.contains(&a) {
                        peers.push(a);
                    }
                }
                // Intro-requests DHT vers les noeuds d'amorcage : la
                // marche discovery peuple l'annuaire partage, le DHT
                // envoie ses propres pings pour remplir ses tables.
                if let Some(dht) = &dht {
                    for addr in &peers {
                        let _ = dht.walk_to(addr).await;
                    }
                }
                if !peers.is_empty() {
                    tracing::info!(
                        count = peers.len(),
                        "bootstrap IPv8 demarre vers les noeuds d'amorcage"
                    );
                    // `RandomWalk` par overlay (BaseLauncher Python
                    // donne `RandomWalk(20)` a toutes les communities
                    // Tribler) : chaque overlay marche sous son propre
                    // prefixe, sinon l'overlay reste vide — le gossip
                    // content-discovery n'a personne a qui parler et
                    // la recherche distante est muette.
                    if let Some(cd) = &cd {
                        let cd = cd.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            cd.run(peers, walker_interval).await;
                        });
                    }
                    if let Some(t) = &tunnel_for_walk {
                        let t = t.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            t.run(peers, walker_interval).await;
                        });
                    }
                    if let Some(dht) = &dht {
                        let dht = dht.clone();
                        let peers = peers.clone();
                        tokio::spawn(async move {
                            dht.walk_run(peers, walker_interval).await;
                        });
                    }
                    d.run(peers, walker_interval).await;
                } else {
                    tracing::warn!("aucun pair de bootstrap n'a pu etre resolu pour IPv8");
                }
            });
        }

        // ADR-0011 : service messagerie — joint le swarm
        // `messaging_hash(pk)` en seeder (cle d'identite) et branche
        // son propre relais `e2e_ready` (demux par info-hash : les
        // circuits BT continuent vers `spawn_e2e_listener`/uTP).
        let messaging = if config.enable_messaging {
            match &tunnel {
                Some(t) => Some(crate::services::messaging::MessagingService::start(
                    t.clone(),
                    key.clone(),
                    onionbit_messaging::MessagingConfig {
                        consent_gate_flagged: config.messaging_consent_flagged,
                        consent_gate_endorsed: config.messaging_consent_endorsed,
                        consent_gate_ledger: config.messaging_consent_ledger,
                        ..onionbit_messaging::MessagingConfig::default()
                    },
                    config.messaging_hops,
                    Some(db.clone()),
                )),
                None => {
                    tracing::warn!(
                        "enable_messaging sans enable_anonymity : messagerie non demarree"
                    );
                    None
                }
            }
        } else {
            None
        };

        // Pont ext → messagerie : le lookup de confiance
        // (`kind=identity`) alimente les gates `consent_gate_*` —
        // sans ext, elles sont inertes.
        if let (Some(m), Some(e)) = (&messaging, &ext) {
            let ext = e.clone();
            m.set_trust_lookup(std::sync::Arc::new(move |pk| {
                ext.trust_info(onionbit_ipv8::ext::attest_kind::IDENTITY, pk)
                    .score
            }));
        }

        let stack = Arc::new(Self {
            endpoint,
            network,
            discovery,
            content_discovery,
            tunnel,
            dht,
            ext,
            key,
            messaging,
            dht_maintenance_stop,
            anon_lanes: Mutex::new(HashMap::new()),
            anon_engine_lock: tokio::sync::Mutex::new(()),
            engine_config: engine_config.clone(),
            downloads_dir: downloads_dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            tasks,
            circuit_bounds: Arc::new((
                AtomicUsize::new(config.min_circuits.max(1) as usize),
                AtomicUsize::new(config.max_circuits.max(1) as usize),
            )),
            socks_listen_ports: config.socks_listen_ports.clone(),
            anon_dht_rate_pps: config.anon_dht_rate_pps,
            anon_dht_client_only: config.anon_dht_client_only,
            anon_dht_backoff_cap_secs: config.anon_dht_backoff_cap_secs,
            swarm_states: Mutex::new(HashMap::new()),
            swarm_lookup: Mutex::new(HashMap::new()),
            pending_peer_sinks: Mutex::new(HashMap::new()),
            hidden_tasks: Mutex::new(Vec::new()),
            self_weak: Mutex::new(std::sync::Weak::new()),
        });
        *stack.self_weak.lock().unwrap() = Arc::downgrade(&stack);
        Ok(stack)
    }

    /// Cle publique IPv8 (hex, affichage API).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.key.public_key().to_bin())
    }

    /// `session.overlays` : instantanes `OverlaySchema` de toutes les
    /// communities chargees, dans l'ordre de chargement Python
    /// (`DiscoveryCommunity` config, puis launchers : content, DHT,
    /// tunnel).
    pub fn overlays_info(&self) -> Vec<onionbit_ipv8::OverlayInfo> {
        let mut out = vec![self.discovery.overlay_info(false)];
        if let Some(c) = &self.content_discovery {
            out.push(c.overlay_info(false));
        }
        if let Some(d) = &self.dht {
            // `DHTDiscoveryCommunity` a son propre `Network` →
            // `is_isolated` vrai (calcule dans `overlay_info`).
            out.push(d.overlay_info());
        }
        if let Some(t) = &self.tunnel {
            out.push(t.overlay_info(false));
        }
        if let Some(e) = &self.ext {
            out.push(e.overlay_info(false));
        }
        out
    }

    /// `GET /api/ipv8/ext` : reglages effectifs et pairs de la
    /// communaute d'extension (ADR-0015), `None` si `ext_enabled =
    /// false` (communaute non creee).
    pub fn ext_info(&self) -> Option<onionbit_ipv8::ext::ExtInfo> {
        self.ext.as_ref().map(|e| e.info())
    }

    /// `POST /api/ipv8/ext/attest` : publie une attestation signee
    /// par la cle du noeud et la pousse aux pairs ext. `None` si la
    /// communaute ext n'est pas creee.
    pub async fn ext_attest(
        &self,
        kind: u8,
        subject: &[u8],
        verdict: u8,
    ) -> Option<std::result::Result<onionbit_ipv8::ext::Attestation, onionbit_ipv8::Ipv8Error>>
    {
        match &self.ext {
            Some(e) => Some(e.publish_attestation(kind, subject, verdict).await),
            None => None,
        }
    }

    /// `GET /api/ipv8/ext/attestations` : les attestations stockees
    /// les plus recentes (borne `attest_list_max` interne), `None` si
    /// ext desactive.
    pub fn ext_attestations(&self) -> Option<Vec<onionbit_ipv8::ext::Attestation>> {
        self.ext.as_ref().map(|e| e.attestations_latest(usize::MAX))
    }

    /// `GET /api/ipv8/ext/trust/{kind}/{subject}` : score de
    /// confiance local du sujet (curateurs suivis + soi), `None` si
    /// ext desactive.
    pub fn ext_trust(&self, kind: u8, subject: &[u8]) -> Option<onionbit_ipv8::ext::TrustInfo> {
        self.ext.as_ref().map(|e| e.trust_info(kind, subject))
    }

    /// Liens du ledger bilateral les plus recemment stockes (Phase
    /// 9c) — `None` si `ext_enabled` off. Le `tx` reste chiffre dans
    /// la forme exposee (anti-crawler : la forme API ne revele que
    /// pk/seq/hash — les volumes restent lisibles par la paire).
    pub fn ext_ledger_links(&self, limit: usize) -> Option<Vec<onionbit_ipv8::ext::LedgerLink>> {
        self.ext.as_ref().map(|e| e.ledger_links(limit))
    }

    /// `enable_overlay_statistics` de `OverlaysEndpoint` : active ou
    /// desactive le comptage pour tous les overlays (`all`) ou celui
    /// nomme `overlay_name` (`__class__.__name__`).
    pub async fn enable_overlay_statistics(
        &self,
        enable: bool,
        overlay_name: Option<&str>,
        all: bool,
    ) {
        for info in self.overlays_info() {
            if all || Some(info.overlay_name) == overlay_name {
                self.endpoint
                    .enable_community_statistics(info.prefix(), enable)
                    .await;
            }
        }
    }

    /// `IsolationEndpoint.add_exit_node` : `walk_to` sur les overlays
    /// tunnel (seule `TunnelCommunity` en Python).
    pub async fn walk_exit_nodes(&self, addr: &UdpAddress) {
        if let Some(t) = &self.tunnel {
            let _ = t.walk_to(addr).await;
        }
    }

    /// `IsolationEndpoint.add_bootstrap_server` : blacklist reseau
    /// (globale + chaque overlay) puis `walk_to`, et ajout de
    /// l'adresse aux `DispersyBootstrapper.ip_addresses` (chez nous :
    /// le pool de bootstrap de la discovery).
    pub async fn add_bootstrap_node(&self, addr: &UdpAddress) {
        // `session.network.blacklist.append` — discovery/content/
        // tunnel partagent `self.network` ; le DHT a le sien.
        self.network.add_blacklist(addr.clone());
        let _ = self.discovery.walk_to(addr).await;
        if let Some(c) = &self.content_discovery {
            let _ = c.walk_to(addr).await;
        }
        if let Some(d) = &self.dht {
            d.network().add_blacklist(addr.clone());
            let _ = d.walk_to(addr).await;
        }
        if let Some(t) = &self.tunnel {
            let _ = t.walk_to(addr).await;
        }
        // `bootstrapper.ip_addresses.append` (overlay Community).
        self.discovery.add_bootstrapper(addr.clone());
    }

    /// `NoBlockDHTEndpoint` : lance `DHTDiscoveryCommunity.connect_peer`
    /// en tache de fond et retourne immediatement. `false` si la
    /// community DHT n'est pas chargee (`dht is None` → 404 Python).
    pub fn connect_peer_noblock(&self, mid: [u8; 20]) -> bool {
        let Some(dht) = &self.dht else {
            return false;
        };
        let dht = dht.clone();
        tokio::spawn(async move {
            match dht.connect_peer(&mid, None).await {
                Ok(_) => tracing::debug!(mid = %hex::encode(mid), "dht connect-peer ok"),
                // `DHTError` Python : loggee, pas propagee au REST.
                Err(e) => {
                    tracing::debug!(error = %e, mid = %hex::encode(mid), "dht connect-peer echoue")
                }
            }
        });
        true
    }

    /// Moteur anonyme pour `hops` sauts (1..=3) : cree la lane a la
    /// demande — proxy SOCKS5 loopback dedie + moteur uTP-only.
    ///
    /// Erreur si l'anonymat n'est pas active ou `hops` hors bornes.
    pub async fn anon_engine(&self, hops: usize) -> Result<BtEngine> {
        if !(1..=MAX_ANON_HOPS).contains(&hops) {
            return Err(CoreError::State("anon_hops doit etre entre 1 et 3".into()));
        }
        if let Some(engine) = self
            .anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| l.engine.clone())
        {
            return Ok(engine);
        }
        // Creation serialisee : sans ce verrou, deux adds concurrents
        // sur la meme lane creaient chacun un moteur et le second
        // `insert` ecrasait le premier — orphelin invisible des
        // lookups (`owner_engine_hops`, `find_download_hex`, restore).
        let _create_guard = self.anon_engine_lock.lock().await;
        // Re-check sous verrou : un create concurrent a pu finir
        // pendant l'acquisition.
        if let Some(engine) = self
            .anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| l.engine.clone())
        {
            return Ok(engine);
        }
        let tunnel = self
            .tunnel
            .clone()
            .ok_or_else(|| CoreError::State("anonymat non active".into()))?;
        let socks = Socks5Server::new(tunnel.clone(), hops);
        // `libtorrent/socks_listen_ports[hops-1]` Tribler : port
        // configure par lane (0 = ephemere). Port configure deja pris
        // -> repli ephemere (comme `Failed to start SOCKS5 servers` +
        // poursuite cote Python, au niveau lane).
        let want_port = self.socks_listen_ports.get(hops - 1).copied().unwrap_or(0);
        let socks_addr = match socks.listen(&format!("127.0.0.1:{want_port}")).await {
            Ok(a) => a,
            Err(e) if want_port != 0 => {
                tracing::warn!(
                    error = %e,
                    hops,
                    port = want_port,
                    "port SOCKS5 anonyme configure indisponible, repli sur port ephemere"
                );
                socks
                    .listen("127.0.0.1:0")
                    .await
                    .map_err(|e2| CoreError::State(format!("socks5 bind: {e2}")))?
            }
            Err(e) => return Err(CoreError::State(format!("socks5 bind: {e}"))),
        };
        // Sockets datagramme tunnelsees (cellules `data`) : en Tribler
        // les lanes anonymes desactivent le TCP (`enable_outgoing_tcp`
        // =False) — les pairs passent en uTP a travers le tunnel, la
        // DHT et les trackers UDP aussi ; seuls les trackers HTTP(S)
        // utilisent le SOCKS5 (`http-request` one-shot).
        let udp_sockets = onionbit_tunnel::tunnel_udp_socket::TunnelUdpSockets::with_dht_policy(
            tunnel.clone(),
            hops,
            socks_addr,
            self.anon_dht_client_only,
            self.engine_config.utp_socket_opts(),
        )
        .map_err(|e| CoreError::State(format!("socket uTP tunnel: {e}")))?;
        // Discipline DHT de la lane (extension Rust — Tribler n'a pas
        // de DHT mainline tunnelisee) : plafond de debit sortant sur
        // la socket borne le pire cas quel que soit le client, quand
        // le backoff `get_peers` (cote librqbit-dht vendored) reduit
        // la cadence des lookups sans progres.
        udp_sockets.dht.set_rate_limit_pps(self.anon_dht_rate_pps);
        let mut cfg = self.engine_config.clone();
        cfg.socks5_proxy = Some(format!("socks5://{socks_addr}"));
        cfg.utp_only = true;
        cfg.enable_dht = true;
        cfg.dht_requery_backoff_cap_secs =
            (self.anon_dht_backoff_cap_secs > 0).then_some(self.anon_dht_backoff_cap_secs);
        // Defense en profondeur derriere le filtre client-only de la
        // socket (et filet si celle-ci est desactivee) : le traitement
        // des requetes DHT entrantes est borne par le meme budget
        // par seconde — chaque requete admise produit au plus une
        // reponse, deja plafonnee par le debit socket. `0` = illimite
        // (meme semantique que `anon_dht_rate_pps`).
        cfg.dht_inbound_queries_per_sec =
            (self.anon_dht_rate_pps > 0).then_some(self.anon_dht_rate_pps as usize);
        // Le bootstrap DHT tunnel ne peut aboutir qu'une fois des
        // circuits READY — attendre la readiness ici bloquerait le
        // demarrage de la lane pour rien (le bootstrap retente en
        // tache de fond).
        cfg.dht_readiness_timeout_secs = 0;
        let utp_transport = udp_sockets.utp_transport.clone();
        cfg.utp_socket = Some(udp_sockets.utp.clone());
        // Le hidden seeding recoit le uTP SYN du downloader en cellule
        // `data` e2e : la lane doit accepter les connexions entrantes
        // sur la socket tunnelsee (sans elle, librqbit_utp cache le
        // SYN faute d'accepteur puis repond RST — jamais de pair).
        // `on_e2e_finished` pyipv8 est le chemin symetrique : cote
        // seeder, le pair arrive par l'ecoute uTP du tunnel.
        cfg.utp_listen_socket = Some(udp_sockets.utp);
        let dht_socket = udp_sockets.dht.clone();
        cfg.dht_socket = Some(std::sync::Arc::new(udp_sockets.dht));
        cfg.udp_tracker_socket = Some(std::sync::Arc::new(udp_sockets.tracker));
        cfg.disable_lsd = true;
        cfg.listen_port = None;
        // Parite Python : le `destination`/`saveas` est global, pas de
        // sous-dossier par saut — une lane anonyme telecharge dans le
        // meme dossier par defaut que le moteur principal (sauf
        // `destination` explicite a l'ajout, qui prevaut par download).
        cfg.output_dir = self.downloads_dir.clone();
        // Fastresume par lane (meme convention que `main`) — un
        // dossier dedie empeche la restauration croisee des torrents
        // anonymes sur le moteur en clair.
        cfg.persistence_dir = Some(self.state_dir.join("rqbit").join(format!("anon{hops}")));
        let engine = BtEngine::start(cfg).await?;
        self.tasks.register(
            Some("Ipv8Stack"),
            &format!("circuit_watchdog_{hops}"),
            Some(CIRCUIT_PROBE_INTERVAL.as_secs_f64()),
        );
        let circuit_watchdog_stop = spawn_circuit_watchdog(
            tunnel.clone(),
            hops,
            engine
                .kill_switch()
                .expect("kill switch absent avec proxy configure"),
            self.circuit_bounds.clone(),
            engine.clone(),
        );
        let lane = AnonLane {
            socks_addr,
            socks,
            engine: engine.clone(),
            utp_transport,
            dht_socket,
            circuit_watchdog_stop,
        };
        self.anon_lanes.lock().unwrap().insert(hops, lane);
        // `monitor_hidden_swarms` + `on_e2e_finished` Python : le
        // suivi des swarms caches et l'injection des pairs e2e sont
        // globaux a la stack, demarres a la premiere lane.
        self.ensure_hidden_tasks(&tunnel);
        tracing::info!(hops, %socks_addr, "lane anonyme creee");
        Ok(engine)
    }

    /// Demarre (une fois) les taches hidden-services de la stack :
    /// le poll `monitor_hidden_swarms` et le relais `e2e_ready` ->
    /// pairs uTP (`on_e2e_finished` Python).
    fn ensure_hidden_tasks(&self, tunnel: &Arc<TunnelCommunity>) {
        let mut tasks = self.hidden_tasks.lock().unwrap();
        if !tasks.is_empty() {
            return;
        }
        let weak = self.self_weak.lock().unwrap().clone();
        self.tasks.register(
            Some("TriblerTunnelCommunity"),
            "monitor_hidden_swarms",
            Some(SWARM_MONITOR_INTERVAL.as_secs_f64()),
        );
        tasks.push(spawn_swarm_monitor(weak.clone(), tunnel.clone()));
        tasks.push(spawn_e2e_listener(weak, tunnel.clone()));
    }

    /// Met a jour a chaud les bornes `min_circuits`/`max_circuits`
    /// partagees avec les watchdogs de lanes (`POST /api/settings` —
    /// comme `self.settings` relu par `monitor_downloads` Python).
    pub fn set_circuit_bounds(&self, min_circuits: usize, max_circuits: usize) {
        self.circuit_bounds
            .0
            .store(min_circuits.max(1), Ordering::Relaxed);
        self.circuit_bounds
            .1
            .store(max_circuits.max(1), Ordering::Relaxed);
    }

    /// Bornes `min_circuits`/`max_circuits` effectives (pour
    /// `effective_config` / `GET /api/settings`).
    pub fn circuit_bounds(&self) -> (usize, usize) {
        (
            self.circuit_bounds.0.load(Ordering::Relaxed),
            self.circuit_bounds.1.load(Ordering::Relaxed),
        )
    }

    /// Lane anonyme pour `hops` sauts (acces interne au transport).
    fn anon_lane(&self, hops: usize) -> Option<(BtEngine, TunnelUdpSocket)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| (l.engine.clone(), l.utp_transport.clone()))
    }

    /// Compteurs de la socket DHT tunnelisee d'une lane (`TunnelUdpSocket::
    /// stats`) : `(tx_msgs, tx_bytes, rx_msgs, rx_bytes, dropped,
    /// rx_queries_dropped)` — surface d'observabilite du budget DHT
    /// anonyme (mesures fingerprinting, tests de regression de
    /// cadence). `None` si la lane `hops` n'existe pas.
    pub fn anon_dht_stats(&self, hops: usize) -> Option<(u64, u64, u64, u64, u64, u64)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .get(&hops)
            .map(|l| l.dht_socket.stats())
    }

    /// Liste des lanes anonymes actives (statistiques API).
    pub fn anon_lanes(&self) -> Vec<(usize, SocketAddr)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .iter()
            .map(|(h, l)| (*h, l.socks_addr))
            .collect()
    }

    /// Moteurs des lanes anonymes actives (pour `downloads()`,
    /// `pause`/`resume`/`remove` cross-moteurs).
    pub fn anon_engines(&self) -> Vec<BtEngine> {
        self.anon_lanes
            .lock()
            .unwrap()
            .values()
            .map(|l| l.engine.clone())
            .collect()
    }

    /// Idem avec le nombre de sauts de chaque lane — la lane est la
    /// verite moteur (contra a `downloads.anon_hops`, l'intention
    /// persistee).
    pub fn anon_engines_with_hops(&self) -> Vec<(usize, BtEngine)> {
        self.anon_lanes
            .lock()
            .unwrap()
            .iter()
            .map(|(h, l)| (*h, l.engine.clone()))
            .collect()
    }

    /// Enregistre le swarm cache d'un magnet encore `pending` : la
    /// decouverte (`do_peer_discovery` -> `swarm_lookup` -> e2e)
    /// demarre avant la materialisation. Sans cela un magnet anonyme
    /// ne pouvait jamais rencontrer un seeder cache : le moniteur
    /// n'observe que les torrents deja crees par le moteur, or rqbit
    /// resout les metadonnees AVANT de creer le torrent — deadlock.
    /// `join_swarm` est idempotent (`join_swarm_with_key` ignore un
    /// re-join identique pour preserver les points d'introduction).
    pub fn register_pending_swarm(&self, real_ih: [u8; 20], hops: usize) {
        let lookup = lookup_info_hash(&real_ih);
        self.swarm_lookup
            .lock()
            .unwrap()
            .insert(lookup, (hops, real_ih));
        if let Some(t) = &self.tunnel {
            t.join_swarm(lookup, hops, false);
        }
    }

    /// Remplace le sink de pairs d'un magnet `pending` — appele a
    /// chaque tentative de resolution (le receveur precedent a ete
    /// consomme par le `peer_rx` rqbit de l'essai en cours).
    pub fn set_pending_peer_sink(
        &self,
        real_ih: [u8; 20],
        tx: tokio::sync::mpsc::UnboundedSender<SocketAddr>,
    ) {
        self.pending_peer_sinks.lock().unwrap().insert(real_ih, tx);
    }

    /// Retire les enregistrements `pending` d'un info-hash reel.
    /// `materialized` = le torrent existe dans le moteur : le
    /// moniteur de swarm reprend le relais (join/leave officiels),
    /// on ne demonte ni le swarm ni le mapping.
    pub fn clear_pending_swarm(&self, real_ih: &[u8; 20], materialized: bool) {
        self.pending_peer_sinks.lock().unwrap().remove(real_ih);
        if materialized {
            return;
        }
        let lookup = lookup_info_hash(real_ih);
        self.swarm_lookup.lock().unwrap().remove(&lookup);
        if let Some(t) = &self.tunnel {
            t.leave_swarm(&lookup);
        }
    }

    /// Liaisons download<->swarm connues du moniteur (equivalent du
    /// `self.download_states` de `monitor_downloads` Python, joint a
    /// `swarm_lookup` pour l'info-hash reel) — diagnostic des
    /// telechargements anonymes. Triee par lookup pour une sortie
    /// stable ; une entree sans etat encore vu (course d'insertion
    /// d'un tick) est omise plutot que d'inventer un etat.
    pub fn swarm_downloads(&self) -> Vec<SwarmDownload> {
        let lookup = self.swarm_lookup.lock().unwrap();
        let states = self.swarm_states.lock().unwrap();
        let mut out: Vec<SwarmDownload> = lookup
            .iter()
            .filter_map(|(lookup_ih, (hops, real_ih))| {
                states
                    .get(&(*hops, *lookup_ih))
                    .copied()
                    .map(|state| SwarmDownload {
                        lookup_info_hash: *lookup_ih,
                        info_hash: *real_ih,
                        hops: *hops,
                        state,
                    })
            })
            .collect();
        out.sort_by_key(|d| d.lookup_info_hash);
        out
    }

    /// Detruit la lane anonyme `hops` : retrait du registre, arret du
    /// watchdog de circuits puis du moteur — les sockets uTP/DHT
    /// tunnelisees et le listener SOCKS5 de la lane sont liberes avec
    /// elle. La prochaine insertion anonyme a `hops` sauts recree une
    /// lane neuve (`get_or_create`, nouveaux ports locaux).
    /// `false` si aucune lane `hops` n'existe.
    ///
    /// Surface de diagnostic/test (P0-17c-5 : destruction de lane sous
    /// capture OS — aucun paquet de l'ancienne lane ne doit survivre).
    /// Les circuits tunnel partages ne sont PAS detruits : ils
    /// appartiennent au communaute, pas a la lane.
    pub async fn remove_anon_lane(&self, hops: usize) -> bool {
        let lane = self.anon_lanes.lock().unwrap().remove(&hops);
        let Some(lane) = lane else {
            return false;
        };
        let _ = lane.circuit_watchdog_stop.send(true);
        // Le listener SOCKS5 de la lane doit mourir avec elle : la
        // boucle `accept` detient son propre `Arc` + le `TcpListener`
        // — sans `shutdown()` le port resterait ouvert et servirait
        // encore les connexions d'une lane "detruite".
        lane.socks.shutdown();
        lane.engine.stop().await;
        tracing::info!(hops, socks = %lane.socks_addr, "lane anonyme detruite");
        true
    }

    /// Arret des moteurs anonymes et de la maintenance DHT
    /// (`Session.shutdown` Python : les overlay tasks sont annulees).
    pub async fn stop(&self) {
        if let Some(tx) = &self.dht_maintenance_stop {
            let _ = tx.send(true);
        }
        for tx in self.hidden_tasks.lock().unwrap().drain(..) {
            let _ = tx.send(true);
        }
        if let Some(m) = &self.messaging {
            m.stop();
        }
        // `exitnode_cache` Python : persiste les noeuds de sortie
        // connus (re-pinges au prochain demarrage).
        if let Some(t) = &self.tunnel {
            save_exitnode_cache(t, &self.state_dir.join(EXITNODE_CACHE_FILE));
        }
        let lanes: Vec<AnonLane> = self
            .anon_lanes
            .lock()
            .unwrap()
            .drain()
            .map(|(_, l)| l)
            .collect();
        for lane in lanes {
            let _ = lane.circuit_watchdog_stop.send(true);
            lane.socks.shutdown();
            lane.engine.stop().await;
        }
    }
}

/// `load_exit_nodes` Python : lit `<pubkey_hex> <addr> <flags>` par
/// ligne. Les entrees invalides sont ignorees (cache best-effort).
fn load_exitnode_cache(path: &Path) -> Vec<(Vec<u8>, SocketAddr, i32)> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pk = hex::decode(it.next()?).ok()?;
            let addr = it.next()?.parse().ok()?;
            let flags = it.next()?.parse().ok()?;
            Some((pk, addr, flags))
        })
        .collect()
}

/// `exitnode_cache` Python (`unload` -> `Network.snapshot` filtre
/// aux exits) : les pairs verifies annoncant un flag de sortie sont
/// persistes avec leurs flags pour etre re-pinges au redemarrage.
fn save_exitnode_cache(tunnel: &TunnelCommunity, path: &Path) {
    use onionbit_network_policy::exit_policy::{
        PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_HTTP, PEER_FLAG_EXIT_IPV8,
    };
    const EXIT_MASK: i32 = PEER_FLAG_EXIT_BT | PEER_FLAG_EXIT_IPV8 | PEER_FLAG_EXIT_HTTP;
    let mut out = String::new();
    for peer in tunnel.network().verified_peers() {
        let flags = tunnel.peer_flags_of(&peer.public_key_bin);
        if flags & EXIT_MASK == 0 {
            continue;
        }
        let Some(addr) = peer.address.as_ref().and_then(|a| a.to_socket_addr()) else {
            continue;
        };
        out.push_str(&format!(
            "{} {addr} {flags}\n",
            hex::encode(&peer.public_key_bin)
        ));
    }
    if out.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, out) {
        tracing::warn!(error = %e, "exitnode_cache: ecriture impossible");
    }
}

/// `monitor_hidden_swarms` de `TriblerTunnelCommunity` : poll des
/// etats de telechargement des lanes anonymes — a chaque transition
/// `join_swarm` (seeder au `Seeding`) / `leave_swarm`, et creation
/// de points d'introduction (`create_introduction_point` +
/// `establish-intro`) quand le torrent est complet.
fn spawn_swarm_monitor(
    stack: std::sync::Weak<Ipv8Stack>,
    tunnel: Arc<TunnelCommunity>,
) -> tokio::sync::watch::Sender<bool> {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWARM_MONITOR_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = rx.changed() => break,
                _ = tick.tick() => {}
            }
            let Some(stack) = stack.upgrade() else { break };
            let mut seen = std::collections::HashSet::new();
            let lanes: Vec<(usize, BtEngine)> = stack
                .anon_lanes
                .lock()
                .unwrap()
                .iter()
                .map(|(h, l)| (*h, l.engine.clone()))
                .collect();
            for (hops, engine) in lanes {
                for stat in engine.list() {
                    let Ok(raw) = hex::decode(&stat.info_hash) else {
                        continue;
                    };
                    let Ok(real_ih) = <[u8; 20]>::try_from(raw.as_slice()) else {
                        continue;
                    };
                    // `get_lookup_info_hash` : le swarm vit sous
                    // l'identite SHA1("tribler anonymous download" +
                    // hex(infohash)) — l'infohash reel reste sur le
                    // download.
                    let lookup = lookup_info_hash(&real_ih);
                    seen.insert((hops, lookup));
                    stack
                        .swarm_lookup
                        .lock()
                        .unwrap()
                        .insert(lookup, (hops, real_ih));
                    let prev = stack
                        .swarm_states
                        .lock()
                        .unwrap()
                        .insert((hops, lookup), stat.state);
                    use onionbit_bittorrent::DownloadState as S;
                    if prev != Some(stat.state) {
                        match stat.state {
                            S::Downloading | S::Initializing | S::Checking => {
                                tunnel.join_swarm(lookup, hops, false);
                            }
                            S::Seeding => {
                                tunnel.join_swarm(lookup, hops, true);
                            }
                            S::Paused | S::Stopped | S::Error => {
                                tunnel.leave_swarm(&lookup);
                            }
                        }
                    }
                    // `monitor_hidden_swarms` pyipv8 : l'appel a
                    // `create_introduction_point` n'est PAS borne a la
                    // transition — il est retente a chaque tick tant
                    // que le swarm `SEEDING` n'a pas son compte de
                    // circuits IP_SEEDER (`info_hash not in ip_hashes`).
                    // Sinon un premier essai echoue (reseau encore vide
                    // de pairs exit) condamnait le hidden seeding pour
                    // toute la session.
                    if stat.state == S::Seeding {
                        ensure_introduction_points(&tunnel, lookup);
                        // Re-annonce DHT periodique (bornee par
                        // `intro_reannounce_interval`) : l'annonce
                        // faite par le point d'introduction peut etre
                        // perdue/non propagee — le seeder re-publie.
                        let t = tunnel.clone();
                        tokio::spawn(async move {
                            t.reannounce_intro_points(lookup).await;
                        });
                    }
                }
            }
            // Downloads disparus ou lanes arretees -> `leave_swarm`.
            let gone: Vec<(usize, [u8; 20])> = stack
                .swarm_states
                .lock()
                .unwrap()
                .keys()
                .filter(|k| !seen.contains(k))
                .copied()
                .collect();
            for key in gone {
                stack.swarm_states.lock().unwrap().remove(&key);
                stack.swarm_lookup.lock().unwrap().remove(&key.1);
                tunnel.leave_swarm(&key.1);
            }
        }
    });
    tx
}

/// Cree des points d'introduction jusqu'a `max_intro_points`
/// circuits `IP_SEEDER` prets/en cours pour le swarm (`new_state ==
/// SEEDING` de `monitor_hidden_swarms`).
pub(crate) fn ensure_introduction_points(tunnel: &Arc<TunnelCommunity>, lookup: [u8; 20]) {
    let lookup_hex = hex::encode(lookup);
    let existing = tunnel
        .circuits_info()
        .iter()
        .filter(|c| {
            c.ctype == onionbit_tunnel::routing::CIRCUIT_TYPE_IP_SEEDER
                && c.info_hash.as_deref() == Some(lookup_hex.as_str())
        })
        .count();
    let target = tunnel.settings.max_intro_points;
    // `required_ip` pyipv8 : `intro_point_peer` epingle impose le
    // DERNIER saut des circuits IP_SEEDER. Pair inconnu/non verifie ->
    // `walk_to` pour le decouvrir, retry au prochain tick (creer sans
    // le pin contournerait le reglage).
    let pinned = match tunnel.settings.intro_point_peer.as_ref() {
        None => None,
        Some(addr) => match tunnel.network().get_verified_by_address(addr) {
            Some(p) => Some(p),
            None => {
                tracing::debug!(
                    addr = ?addr,
                    "intro_point_peer pas encore verifie : walk_to, retry au prochain tick"
                );
                let t = tunnel.clone();
                let a = addr.clone();
                tokio::spawn(async move {
                    let _ = t.walk_to(&a).await;
                });
                return;
            }
        },
    };
    for _ in existing..target {
        let tunnel = tunnel.clone();
        let required = pinned.clone();
        tokio::spawn(async move {
            match tunnel
                .create_introduction_point(lookup, required.as_ref())
                .await
            {
                Ok(cid) => {
                    let timeout = tunnel.settings.circuit_timeout.as_millis() as u64;
                    if tunnel.wait_circuit_ready(cid, timeout).await.is_ok() {
                        match tunnel.send_establish_intro(cid, lookup).await {
                            Ok(rx) => {
                                let _ = rx.await;
                            }
                            Err(e) => {
                                tracing::debug!(cid, error = %e, "establish-intro non envoye");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "create_introduction_point: pas de circuit IP_SEEDER");
                }
            }
        });
    }
}

/// `on_e2e_finished` Python : a la liaison d'un circuit e2e, injecte
/// le pair dans le download concerne.
///
/// - `RP_DOWNLOADER` (on telecharge) : l'adresse factice est epinglee
///   sur le circuit e2e dans la socket uTP de la lane, les cellules
///   entrantes y sont injectees et l'adresse est passee a
///   `Download::add_peer` (`dl.add_peer(circuit_id_to_ip(cid), 1024)`
///   Python — meme adresse factice pour les deux roles).
/// - `RP_SEEDER` (on seede) : l'adresse factice
///   `circuit_id_to_ip(cid):1024` est epinglee sur le circuit dans le
///   transport uTP de la lane et les cellules `data` entrantes y
///   sont injectees — les reponses repartent dans le meme circuit
///   (`set_udp_associate_default_remote` Python).
fn spawn_e2e_listener(
    stack: std::sync::Weak<Ipv8Stack>,
    tunnel: Arc<TunnelCommunity>,
) -> tokio::sync::watch::Sender<bool> {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let mut e2e = tunnel.e2e_ready();
    tokio::spawn(async move {
        loop {
            let (cid, lookup) = tokio::select! {
                _ = rx.changed() => break,
                ev = e2e.recv() => match ev {
                    Ok(v) => v,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                },
            };
            let Some(stack) = stack.upgrade() else { break };
            let Some((hops, real_ih)) = stack.swarm_lookup.lock().unwrap().get(&lookup).copied()
            else {
                continue;
            };
            let Some((engine, utp)) = stack.anon_lane(hops) else {
                continue;
            };
            let ctype = stack
                .tunnel
                .as_ref()
                .and_then(|t| {
                    t.circuits_info()
                        .into_iter()
                        .find(|c| c.circuit_id == cid)
                        .map(|c| c.ctype)
                })
                .unwrap_or_default();
            let fake = SocketAddr::V4(std::net::SocketAddrV4::new(
                circuit_id_to_ip(cid),
                CIRCUIT_ID_PORT,
            ));
            match ctype.as_str() {
                // `on_e2e_finished` pyipv8 : `add_peer` recoit
                // `circuit_id_to_ip(cid):1024` dans les DEUX roles —
                // la socket uTP de la lane est une `TunnelUdpSocket`
                // (pas d'UDP reel) : `dial` loopback serait un
                // datagramme `data` vers l'exit, jamais vu en local.
                // Le pin impose le circuit e2e lie et les cellules
                // entrantes sont injectees sous l'adresse factice.
                onionbit_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER
                | onionbit_tunnel::routing::CIRCUIT_TYPE_RP_SEEDER => {
                    utp.pin_circuit(fake, cid);
                    let mut data_rx = tunnel.subscribe_circuit_data(cid);
                    let utp = utp.clone();
                    let fake2 = fake;
                    tokio::spawn(async move {
                        let mut n = 0u64;
                        while let Some(msg) = data_rx.recv().await {
                            // Le circuit e2e peut transporter autre
                            // chose que de l'uTP (mis-routage, bruit
                            // du pair) : filtrer par forme comme le
                            // fait `data_rx` de `TunnelUdpSocket`.
                            if !onionbit_network_policy::exit_policy::could_be_utp(&msg.data) {
                                continue;
                            }
                            n += 1;
                            if n <= 16 || n.is_multiple_of(64) {
                                // Entete uTP : seq_nr/ack_nr (offsets 16 et
                                // 18) + 8 premiers octets de payload — pour
                                // distinguer desordre/rejeu de corruption.
                                let d = &msg.data;
                                let seq = u16::from_be_bytes([d[16], d[17]]);
                                let ack = u16::from_be_bytes([d[18], d[19]]);
                                let pay = &d[20..d.len().min(28)];
                                tracing::debug!(
                                    circuit_id = cid,
                                    n,
                                    len = d.len(),
                                    head = %hex::encode(&d[..d.len().min(8)]),
                                    seq_nr = seq,
                                    ack_nr = ack,
                                    payload_head = %hex::encode(pay),
                                    "e2e -> injection uTP"
                                );
                            }
                            utp.inject_incoming(msg.data, fake2);
                        }
                    });
                    let mut added = false;
                    if ctype == onionbit_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER {
                        if let Some(dl) = engine.get_by_hash(&real_ih) {
                            added = dl.add_peer(fake);
                        } else if let Some(tx) =
                            stack.pending_peer_sinks.lock().unwrap().get(&real_ih)
                        {
                            // Magnet encore en resolution : le torrent
                            // n'existe pas dans le moteur — l'adresse
                            // factice part dans le `peer_rx` de
                            // `resolve_magnet` via `extra_peers_rx`.
                            added = tx.send(fake).is_ok();
                        }
                    }
                    tracing::debug!(
                        circuit_id = cid,
                        ctype = %ctype,
                        %fake,
                        add_peer = added,
                        info_hash = %hex::encode(real_ih),
                        "e2e listener : lane branchee"
                    );
                }
                _ => {}
            }
        }
    });
    tx
}

/// Charge la cle IPv8 depuis `path` ou en cree une nouvelle (format
/// binaire `LibNaCLSK:`).
fn load_or_create_key(path: &Path) -> Result<LibNaClSecretKey> {
    match std::fs::read(path) {
        Ok(data) => LibNaClSecretKey::from_bin(&data).map_err(CoreError::from),
        Err(_) => {
            let key = LibNaClSecretKey::generate();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, key.to_bin())?;
            Ok(key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_format::mdblob::*;

    fn test_provider(notifier: crate::notifier::Notifier) -> SessionContentProvider {
        SessionContentProvider {
            db: std::sync::Arc::new(Database::memory().unwrap()),
            notifier,
            remote_queries_in_progress: std::sync::atomic::AtomicUsize::new(0),
            max_response_size: 100,
            max_payload_size: 1300,
            healths_cache: Mutex::new(HashMap::new()),
            healths_cache_ttl: std::time::Duration::from_secs(60),
            seen_nodes: Mutex::new(std::collections::HashSet::new()),
            gossip: Mutex::new(GossipMemory::default()),
        }
    }

    fn torrent_entry(infohash: [u8; 20], pk: [u8; 64], id: u64) -> MetadataEntry {
        MetadataEntry::RegularTorrent(TorrentMetadataPayload {
            node: ChannelNodePayload {
                header: SignedPayloadHeader::new(types::REGULAR_TORRENT, 0, pk)
                    .with_signature([0xAA; 64]),
                id,
                origin_id: 0,
                timestamp: 1_700_000_500,
            },
            infohash,
            size: 123,
            torrent_date: 1_700_000_000,
            title: "un titre".into(),
            tags: String::new(),
            tracker_info: String::new(),
        })
    }

    fn health(infohash: [u8; 20], seeders: u32, leechers: u32) -> HealthInfo {
        HealthInfo {
            infohash,
            seeders,
            leechers,
            last_check: 42,
            tracker: String::new(),
        }
    }

    /// La sante gossip arrive avant la reponse distante : le resultat
    /// pousse a l'UI doit porter les vrais compteurs (comportement
    /// `to_simple_dict` Python — jointure `torrent_state`).
    #[tokio::test]
    async fn sante_gossip_remplit_les_resultats_distants() {
        let provider = test_provider(crate::notifier::Notifier::new());
        let ih = [7u8; 20];

        let unknown = provider.process_health(&[health(ih, 3922, 40)]).await;
        assert_eq!(unknown, vec![ih]);
        assert_eq!(provider.known_health(&ih).unwrap().seeders, 3922);

        let raw = encode_entry_presigned(&torrent_entry(ih, [1u8; 64], 1)).unwrap();
        let results = provider.process_select_response(&lz4_frame(&raw)).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["num_seeders"], 3922);
        assert_eq!(results[0]["num_leechers"], 40);
        assert_eq!(results[0]["last_tracker_check"], 42);
        // `to_simple_dict` Python : `updated` = `updated_on` du noeud
        // (timestamp signe) — la colonne Date de l'UI lit ce champ.
        assert_eq!(results[0]["updated"], 1_700_000_500);
        assert_eq!(results[0]["created"], 1_700_000_000);
    }

    /// `torrent_health_updated` n'est emis que pour les infohashes
    /// deja pousses a l'UI — pas pour le reste du gossip.
    #[tokio::test]
    async fn health_updated_seulement_pour_les_affiches() {
        let notifier = crate::notifier::Notifier::new();
        let mut rx = notifier.subscribe();
        let provider = test_provider(notifier);
        let shown = [1u8; 20];
        let hidden = [2u8; 20];

        let raw = encode_entry_presigned(&torrent_entry(shown, [9u8; 64], 1)).unwrap();
        assert_eq!(
            provider
                .process_select_response(&lz4_frame(&raw))
                .await
                .len(),
            1
        );

        provider
            .process_health(&[health(shown, 10, 2), health(hidden, 5, 1)])
            .await;

        let mut shown_updates = 0;
        while let Ok(n) = rx.try_recv() {
            if let crate::notifier::Notification::TorrentHealthUpdated {
                infohash,
                seeders,
                leechers,
            } = n
            {
                assert_eq!(infohash, hex::encode(shown));
                assert_eq!((seeders, leechers), (10, 2));
                shown_updates += 1;
            }
        }
        assert_eq!(shown_updates, 1);
    }

    /// Les ensembles memoire restent bornes : au-dela de la borne,
    /// la map de santes evince plutot que de croitre sans fin.
    #[tokio::test]
    async fn memoire_gossip_bornee() {
        let mut g = GossipMemory::default();
        for i in 0..GOSSIP_HEALTH_CAP + 10 {
            let mut ih = [0u8; 20];
            ih[..8].copy_from_slice(&(i as u64).to_be_bytes());
            let fresh = now_unix();
            g.insert_health(&HealthInfo {
                infohash: ih,
                seeders: 1,
                leechers: 0,
                // last_check croissant : toutes les entrees restent
                // « fraiches » (<24 h) — l'eviction tombe sur le
                // chemin pathologique (moitie la plus ancienne).
                last_check: fresh,
                tracker: String::new(),
            });
        }
        assert!(g.healths.len() <= GOSSIP_HEALTH_CAP);
    }
}
