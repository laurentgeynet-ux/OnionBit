//! Structures de routage (port de `messaging/anonymization/tunnel.py`
//! et `ipv8-rust-tunnels/src/routing/`) : `Hop`, `Circuit`,
//! `RelayRoute`, `RendezvousPoint`, `IntroductionPoint`, `Swarm`.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::session::SessionKeys;
use tribler_ipv8::UdpAddress;

/// `PEER_FLAG_RELAY` : le pair accepte de relayer.
pub const PEER_FLAG_RELAY: i32 = 1;
/// `PEER_FLAG_EXIT_BT` : le pair accepte de sortir du trafic BT.
pub const PEER_FLAG_EXIT_BT: i32 = 2;
/// `PEER_FLAG_EXIT_IPV8`.
pub const PEER_FLAG_EXIT_IPV8: i32 = 4;
/// `PEER_FLAG_SPEED_TEST`.
pub const PEER_FLAG_SPEED_TEST: i32 = 8;

/// `PEER_SOURCE_UNKNOWN`.
pub const PEER_SOURCE_UNKNOWN: u8 = 0;
/// `PEER_SOURCE_DHT`.
pub const PEER_SOURCE_DHT: u8 = 1;
/// `PEER_SOURCE_PEX`.
pub const PEER_SOURCE_PEX: u8 = 2;

/// `CIRCUIT_TYPE_DATA`.
pub const CIRCUIT_TYPE_DATA: &str = "DATA";
/// `CIRCUIT_TYPE_IP_SEEDER`.
pub const CIRCUIT_TYPE_IP_SEEDER: &str = "IP_SEEDER";
/// `CIRCUIT_TYPE_RP_SEEDER`.
pub const CIRCUIT_TYPE_RP_SEEDER: &str = "RP_SEEDER";
/// `CIRCUIT_TYPE_RP_DOWNLOADER`.
pub const CIRCUIT_TYPE_RP_DOWNLOADER: &str = "RP_DOWNLOADER";

/// `CIRCUIT_STATE_READY`.
pub const CIRCUIT_STATE_READY: &str = "READY";
/// `CIRCUIT_STATE_EXTENDING`.
pub const CIRCUIT_STATE_EXTENDING: &str = "EXTENDING";
/// `CIRCUIT_STATE_CLOSING`.
pub const CIRCUIT_STATE_CLOSING: &str = "CLOSING";

/// `CIRCUIT_ID_PORT` : adresse "de sortie" factice cote initiateur.
pub const CIRCUIT_ID_PORT: u16 = 1024;

/// `DESTROY_REASON_UNKNOWN`.
pub const DESTROY_REASON_UNKNOWN: u16 = 1;
/// `DESTROY_REASON_SHUTDOWN`.
pub const DESTROY_REASON_SHUTDOWN: u16 = 2;
/// `DESTROY_REASON_UNNEEDED`.
pub const DESTROY_REASON_UNNEEDED: u16 = 4;

/// Inactivite max avant destruction d'un circuit
/// (`remove_circuit "no activity"` — le Python n'a pas de constante
/// unique ; TunnelSettings.circuit_timeout ~= 60 s cote tunnels Rust).
const CIRCUIT_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);

/// `Hop` : un saut de circuit.
#[derive(Debug)]
pub struct Hop {
    /// Cle publique binaire du pair (`key_to_bin`).
    pub public_key_bin: Vec<u8>,
    /// Adresse du saut.
    pub address: Option<UdpAddress>,
    /// Cles de session partagees avec ce saut.
    pub session_keys: SessionKeys,
}

impl Clone for Hop {
    fn clone(&self) -> Self {
        Self {
            public_key_bin: self.public_key_bin.clone(),
            address: self.address.clone(),
            session_keys: self.session_keys.clone(),
        }
    }
}

/// `RoutingObject` : stats communes circuit/relais/sortie.
#[derive(Debug)]
pub struct RoutingObject {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// Derniere activite (`beat_heart`).
    pub last_incoming: Instant,
    /// Derniere activite sortante.
    pub last_outgoing: Instant,
    /// Octets montants.
    pub bytes_up: u64,
    /// Octets descendants.
    pub bytes_down: u64,
    /// `creation_time`.
    pub creation_time: Instant,
}

impl RoutingObject {
    /// `__init__`.
    pub fn new(circuit_id: u32) -> Self {
        let now = Instant::now();
        Self {
            circuit_id,
            last_incoming: now,
            last_outgoing: now,
            bytes_up: 0,
            bytes_down: 0,
            creation_time: now,
        }
    }

    /// `beat_heart`.
    pub fn beat_heart(&mut self) {
        self.last_incoming = Instant::now();
    }

    /// Inactif depuis plus que `CIRCUIT_INACTIVITY_TIMEOUT`.
    pub fn is_inactive(&self) -> bool {
        self.last_incoming.elapsed() > CIRCUIT_INACTIVITY_TIMEOUT
            && self.last_outgoing.elapsed() > CIRCUIT_INACTIVITY_TIMEOUT
    }
}

/// `Circuit` : circuit en cours de construction ou pret.
#[derive(Debug)]
pub struct Circuit {
    /// Base commune.
    pub base: RoutingObject,
    /// Nombre de sauts vise (`goal_hops`).
    pub goal_hops: usize,
    /// Type (`CIRCUIT_TYPE_*`).
    pub ctype: String,
    /// Sauts verifies (avec cles de session).
    pub hops: Vec<Hop>,
    /// `unverified_hop` : saut en cours d'ajout (create envoye).
    pub unverified_hop: Option<Hop>,
    /// `unverified_hop` DH secret ephemere (pour
    /// `verify_and_generate_shared_secret`).
    pub unverified_dh_secret: Option<Vec<u8>>,
    /// Identifiant de la requete `create`/`extend` en vol
    /// (`RetryRequestCache` simplifie).
    pub pending_identifier: Option<u16>,
    /// `e2e` : circuit de bout en bout (hidden services).
    pub e2e: bool,
    /// `info_hash` associe (swarm).
    pub info_hash: Option<[u8; 20]>,
    /// `required_exit` : sortie requise.
    pub required_exit: Option<Vec<u8>>,
    /// Etat force a `CLOSING`.
    closing: Option<String>,
}

impl Circuit {
    /// `Circuit.__init__`.
    pub fn new(circuit_id: u32, goal_hops: usize, ctype: &str, info_hash: Option<[u8; 20]>) -> Self {
        Self {
            base: RoutingObject::new(circuit_id),
            goal_hops,
            ctype: ctype.to_string(),
            hops: Vec::new(),
            unverified_hop: None,
            unverified_dh_secret: None,
            pending_identifier: None,
            e2e: false,
            info_hash,
            required_exit: None,
            closing: None,
        }
    }

    /// `add_hop`.
    pub fn add_hop(&mut self, hop: Hop) {
        self.hops.push(hop);
        self.unverified_hop = None;
        self.unverified_dh_secret = None;
    }

    /// Premier saut (`circuit.hop` — celui auquel on envoie les
    /// cellules).
    pub fn first_hop(&self) -> Option<&Hop> {
        self.hops.first()
    }

    /// `circuit.state` Python.
    pub fn state(&self) -> &'static str {
        if self.closing.is_some() {
            CIRCUIT_STATE_CLOSING
        } else if self.hops.len() < self.goal_hops || self.unverified_hop.is_some() {
            CIRCUIT_STATE_EXTENDING
        } else {
            CIRCUIT_STATE_READY
        }
    }

    /// `close`.
    pub fn close(&mut self, info: &str) {
        self.closing = Some(info.to_string());
    }

    /// `ready` : circuit utilisable (pret et pas en fermeture).
    pub fn ready(&self) -> bool {
        self.state() == CIRCUIT_STATE_READY
    }
}

/// `RelayRoute` : correspondance de relais.
#[derive(Debug)]
pub struct RelayRoute {
    /// Base commune.
    pub base: RoutingObject,
    /// Saut (pair + cles de session).
    pub hop: Hop,
    /// Direction du relais (`FORWARD`/`BACKWARD` de la direction des
    /// donnees vues par ce relais).
    pub direction: tribler_crypto::ipv8::session::Direction,
    /// `rendezvous_relay`.
    pub rendezvous_relay: bool,
    /// `sock_addr` du saut precedent (pour renvoyer les reponses).
    pub prev_addr: Option<SocketAddr>,
}

/// `RendezvousPoint`.
#[derive(Debug)]
pub struct RendezvousPoint {
    /// Circuit associe.
    pub circuit: u32,
    /// Cookie de rendez-vous.
    pub cookie: [u8; 20],
    /// Pret a etre lie.
    pub ready: bool,
}

/// `IntroductionPoint`.
#[derive(Debug, Clone)]
pub struct IntroductionPoint {
    /// Adresse du pair.
    pub address: UdpAddress,
    /// Cle publique du point d'introduction.
    pub peer_key: Vec<u8>,
    /// Cle publique du seeder (`seeder_pk`).
    pub seeder_pk: Vec<u8>,
    /// `source` (PEER_SOURCE_*).
    pub source: u8,
}

impl PartialEq for IntroductionPoint {
    /// `__eq__` Python : (peer, seeder_pk).
    fn eq(&self, other: &Self) -> bool {
        self.peer_key == other.peer_key && self.seeder_pk == other.seeder_pk
    }
}
impl Eq for IntroductionPoint {}
impl std::hash::Hash for IntroductionPoint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.peer_key.hash(state);
        self.seeder_pk.hash(state);
    }
}
