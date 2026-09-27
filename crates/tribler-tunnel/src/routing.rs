//! Structures de routage (port de `messaging/anonymization/tunnel.py`
//! et `ipv8-rust-tunnels/src/routing/`) : `Hop`, `Circuit`,
//! `RelayRoute`, `RendezvousPoint`, `IntroductionPoint`, `Swarm`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::session::SessionKeys;
use tribler_ipv8::UdpAddress;

// Flags de service du protocole de tunnels : la source de verite
// est `tribler-network-policy::exit_policy` (politique de sortie
// appliquee dans `exit_data`).
pub use tribler_network_policy::exit_policy::{
    PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_HTTP, PEER_FLAG_EXIT_IPV8, PEER_FLAG_RELAY,
    PEER_FLAG_SPEED_TEST,
};

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
/// Un pair cache est vu par le client (libtorrent/rqbit) comme
/// `circuit_id_to_ip(circuit_id):CIRCUIT_ID_PORT` — le SOCKS5 decode
/// le circuit_id depuis l'IPv4 (`select_circuit` dans
/// `ipv8-rust-tunnels`).
pub const CIRCUIT_ID_PORT: u16 = 1024;

/// `ip_to_circuit_id` (`packet.rs` des tunnels Rust) : l'IPv4 factice
/// encode le `circuit_id` en big-endian.
pub fn ip_to_circuit_id(ip: &std::net::Ipv4Addr) -> u32 {
    u32::from_be_bytes(ip.octets())
}

/// `circuit_id_to_ip` : adresse IPv4 factice representant le circuit
/// e2e dans les frames SOCKS5.
pub fn circuit_id_to_ip(circuit_id: u32) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::from(circuit_id.to_be_bytes())
}

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
#[derive(Debug, Clone)]
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

/// `unverified_hop` Python : saut en cours d'ajout, avant que le
/// `created`/`extended` ne revele les cles de session (le DH secret
/// ephemere est conserve pour `verify_and_generate_shared_secret`).
#[derive(Debug)]
pub struct UnverifiedHop {
    /// Cle publique binaire du saut.
    pub public_key_bin: Vec<u8>,
    /// Adresse du saut.
    pub address: Option<UdpAddress>,
    /// Secret DH ephemere local (32 octets).
    pub dh_secret: [u8; 32],
    /// `identifier` du create/extend en vol (`packet_identifier`).
    pub identifier: u16,
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
    pub unverified_hop: Option<UnverifiedHop>,
    /// `e2e` : circuit de bout en bout (hidden services).
    pub e2e: bool,
    /// `info_hash` associe (swarm).
    pub info_hash: Option<[u8; 20]>,
    /// `required_exit` : sortie requise.
    pub required_exit: Option<Vec<u8>>,
    /// `relay_early_count` : cellules deja relachees en `relay_early`
    /// (`send_cell` Python le pose si `< max_relay_early`).
    pub relay_early_count: u8,
    /// `hs_session_keys` : couche e2e supplementaire (hidden services,
    /// posee a la liaison `linked-e2e` — `crypto.py` `outgoing_crypto`
    /// /`incoming_crypto`).
    pub hs_session_keys: Option<SessionKeys>,
    /// `exit_flags` (`ipv8-rust-tunnels` `Circuit.exit_flags`) : flags
    /// de service du dernier saut, si connus (selection des circuits
    /// compatibles HTTP).
    pub exit_flags: i32,
    /// Etat force a `CLOSING`.
    closing: Option<String>,
}

impl Circuit {
    /// `Circuit.__init__`.
    pub fn new(
        circuit_id: u32,
        goal_hops: usize,
        ctype: &str,
        info_hash: Option<[u8; 20]>,
    ) -> Self {
        Self {
            base: RoutingObject::new(circuit_id),
            goal_hops,
            ctype: ctype.to_string(),
            hops: Vec::new(),
            unverified_hop: None,
            e2e: false,
            info_hash,
            required_exit: None,
            relay_early_count: 0,
            hs_session_keys: None,
            exit_flags: 0,
            closing: None,
        }
    }

    /// `add_hop`.
    pub fn add_hop(&mut self, hop: Hop) {
        self.hops.push(hop);
        self.unverified_hop = None;
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

/// `RelayRoute` : correspondance de relais. Mappe un circuit_id
/// entrant (cle de la table) vers le circuit_id sortant
/// (`base.circuit_id`) et le saut cible (`hop` — pair vers lequel la
/// cellule est envoyee : aval pour FORWARD, amont pour BACKWARD).
#[derive(Debug, Clone)]
pub struct RelayRoute {
    /// Base commune (`circuit_id` = id sortant).
    pub base: RoutingObject,
    /// Saut cible (pair suivant + cles de session amont-partagees).
    pub hop: Hop,
    /// Direction du flux sur le circuit entrant (`FORWARD` = la
    /// cellule va vers la sortie -> decrypt une couche ; `BACKWARD` =
    /// retour vers l'initiateur -> encrypt une couche).
    pub direction: tribler_crypto::ipv8::session::Direction,
    /// `rendezvous_relay` : transforme FORWARD->BACKWARD au point de
    /// rendez-vous (hidden services).
    pub rendezvous_relay: bool,
    /// Compteur de cellules relayees en `relay_early` (borne
    /// `max_relay_early` Python).
    pub relay_early_count: u8,
}

/// `RendezvousPoint`.
#[derive(Debug)]
pub struct RendezvousPoint {
    /// Circuit associe.
    pub circuit: u32,
    /// Cookie de rendez-vous.
    pub cookie: [u8; 20],
    /// Adresse du point de rendez-vous (`rendezvous_point_addr` de la
    /// reponse, `rp.address` Python).
    pub address: Option<UdpAddress>,
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

/// `Swarm` (hidden_services.py) : swarm attache a un `info_hash` —
/// cote seeder (`seeder_sk` present) ou downloader.
pub struct Swarm {
    /// `info_hash` (SHA-1 du torrent).
    pub info_hash: [u8; 20],
    /// `hops` : sauts des circuits du swarm.
    pub hops: usize,
    /// `seeder_sk` : cle de service du seeder (`None` cote
    /// downloader).
    pub seeder_sk: Option<tribler_crypto::ipv8::keys::LibNaClSecretKey>,
    /// `connections` : circuit_id e2e -> point d'introduction utilise.
    pub connections: HashMap<u32, IntroductionPoint>,
    /// Retentative idempotente (`RequestCache` a retry de pyipv8, cote
    /// downloader) : etape + horodatage de la requete e2e en cours par
    /// point d'introduction. Une retentative re-expedie le MEME
    /// `create-e2e`/`link-e2e` plutot qu'un nouveau handshake qui
    /// creerait un second `RP_SEEDER` cote seeder — sauf si l'etape
    /// est perimee (`PENDING_E2E_TTL`), auquel cas on abandonne cette
    /// tentative et on repart sur un handshake neuf (comme la
    /// reference sans retry, qui reussit toujours ainsi) plutot que de
    /// re-emettre indefiniment un cote qui ne progresse plus.
    pub pending_e2e: HashMap<IntroductionPoint, (PendingE2e, Instant)>,
    /// Dedup (cote seeder) : paquet `created-e2e` deja emis par
    /// (`identifier`, demandeur). Une retransmission du meme
    /// `create-e2e` recoit la copie cachee au lieu de creer un second
    /// circuit `RP_SEEDER`.
    pub seen_e2e: HashMap<(u16, UdpAddress), Vec<u8>>,
    /// Reservation (cote seeder) : `create-e2e` en cours de traitement
    /// par (`identifier`, demandeur) — la reponse n'est pas encore
    /// dans `seen_e2e`, le doublon est ignore. Retire quand le
    /// traitement se termine (succes ou echec).
    pub in_flight_e2e: HashSet<(u16, UdpAddress)>,
}

/// Etape d'une requete e2e en cours (`Swarm::pending_e2e`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingE2e {
    /// `create-e2e` emis, `created-e2e` attendu (`e2e_requests`).
    Create(u16),
    /// `created-e2e` recu : circuit `RP_DOWNLOADER` en construction,
    /// `link-e2e` pas encore emis — une retentative attend sans rien
    /// re-expedier.
    Building,
    /// `link-e2e` emis, `linked-e2e` attendu (`link_requests`).
    Link(u16),
}

impl Swarm {
    /// `Swarm.__init__`.
    pub fn new(
        info_hash: [u8; 20],
        hops: usize,
        seeder_sk: Option<tribler_crypto::ipv8::keys::LibNaClSecretKey>,
    ) -> Self {
        Self {
            info_hash,
            hops,
            seeder_sk,
            connections: HashMap::new(),
            pending_e2e: HashMap::new(),
            seen_e2e: HashMap::new(),
            in_flight_e2e: HashSet::new(),
        }
    }
}

/// Retourne les circuits e2e prets du swarm (`get_circuits` Python).
impl Swarm {
    /// Ids des circuits e2e connectes et prets.
    pub fn e2e_circuits(&self) -> Vec<u32> {
        self.connections.keys().copied().collect()
    }
}
