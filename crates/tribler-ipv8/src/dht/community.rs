//! `DHTCommunity` + `DHTDiscoveryCommunity` pyipv8 (equivalent de
//! `dht/community.py` et `dht/discovery.py`), fusionnees en une seule
//! struct : les 10 messages partagent le meme `community_id`
//! `8d0be1845d74d175f178197cad001591d04d73cc`.
//!
//! Fonctions portees :
//! - ping/pong DHT (msgs 1-2) avec metriques de noeud (rtt, failed) ;
//! - store/find de valeurs (msgs 3-6) avec jetons anti-spoofing
//!   (`sha1(str(node) + secret)`), crawl iteratif borne
//!   (`MAX_CRAWL_NODES/REQUESTS/TASKS`), puncture-request ;
//! - store-peer / connect-peer (msgs 7-10, DHTDiscoveryCommunity) ;
//! - maintenance : churn (purge BAD, ping periodique), tokens, valeurs
//!   expirees, refresh de buckets (>15 min).

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use tribler_crypto::hash::sha1;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;

use crate::address::UdpAddress;
use crate::dht::payloads::{self, msg};
use crate::dht::routing::{
    address_ip_port, calc_node_id, distance, Node, RoutingTable, NODE_STATUS_BAD,
};
use crate::dht::storage::Storage;
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet};
use crate::payloads::{Payload, PunctureRequestPayload};
use crate::peer::{Network, Peer};
use crate::serializer::{Reader, Writer};
use crate::CommunityId;

/// `community_id` de la `DHTCommunity` pyipv8 (inchange — partagee
/// aussi par `DHTDiscoveryCommunity`).
pub const DHT_COMMUNITY_ID: CommunityId = [
    0x8d, 0x0b, 0xe1, 0x84, 0x5d, 0x74, 0xd1, 0x75, 0xf1, 0x78, 0x19, 0x7c, 0xad, 0x00, 0x15, 0x91,
    0xd0, 0x4d, 0x73, 0xcc,
];

/// `PING_INTERVAL` Python (s).
pub const PING_INTERVAL: f64 = 25.0;
/// `TOKEN_EXPIRATION_TIME` (s).
pub const TOKEN_EXPIRATION_TIME: f64 = 600.0;
/// `MAX_ENTRY_AGE` (s).
pub const MAX_ENTRY_AGE: f64 = 3600.0;
/// `MAX_CRAWL_NODES`.
pub const MAX_CRAWL_NODES: usize = 8;
/// `MAX_CRAWL_REQUESTS`.
pub const MAX_CRAWL_REQUESTS: usize = 24;
/// `MAX_CRAWL_TASKS`.
pub const MAX_CRAWL_TASKS: usize = 4;
/// `MAX_VALUES_IN_STORE`.
pub const MAX_VALUES_IN_STORE: usize = 8;
/// `MAX_VALUES_IN_FIND`.
pub const MAX_VALUES_IN_FIND: usize = 8;
/// `MAX_NODES_IN_FIND`.
pub const MAX_NODES_IN_FIND: usize = 8;
/// `TARGET_NODES`.
pub const TARGET_NODES: usize = 8;
/// Timeout par defaut d'une requete (`Request.timeout` Python : 5 s).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout des find-requests (`timeout=2.0` Python).
pub const FIND_TIMEOUT: Duration = Duration::from_secs(2);
/// Buckets non modifies depuis >15 min → refresh (`node_maintenance`).
const BUCKET_REFRESH_AGE: f64 = 15.0 * 60.0;

/// `DHTValue` Python : `(data, public_key | None)`.
pub type DhtValue = (Vec<u8>, Option<Vec<u8>>);

/// `(valeurs, noeuds)` d'une find-response decodee.
type FindResponseData = (Vec<Vec<u8>>, Vec<Arc<Node>>);
/// Reponses d'un crawl : `(emetteur, contenu)`.
type CrawlResponses = Vec<(Arc<Node>, FindResponseData)>;
/// Resultat de `contact_node` : `Some((noeud, valeurs, noeuds))`.
type ContactResult = Option<(Arc<Node>, Vec<Vec<u8>>, Vec<Arc<Node>>)>;
/// `tokens[node_id] = (timestamp, token)`.
type TokenMap = Mutex<HashMap<[u8; 20], (f64, [u8; 20])>>;

/// Classe d'adresse (cle des `routing_tables`/`storages` Python qui
/// indexent par `type(address)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AddrClass {
    /// `UDPv4Address`.
    V4,
    /// `UDPv6Address`.
    V6,
    /// `DomainAddress`.
    Domain,
}

impl AddrClass {
    fn of(addr: &UdpAddress) -> Self {
        match addr {
            UdpAddress::Ipv4(_) => Self::V4,
            UdpAddress::Ipv6(_) => Self::V6,
            UdpAddress::Domain(..) => Self::Domain,
        }
    }
}

/// Erreur DHT (equivalent de `DHTError`).
#[derive(Debug, thiserror::Error)]
pub enum DhtError {
    /// Timeout de requete.
    #[error("timeout de requete DHT ({0})")]
    Timeout(&'static str),
    /// Aucun noeud disponible.
    #[error("aucun noeud DHT disponible : {0}")]
    NoNodes(&'static str),
    /// Valeur trop grande.
    #[error("valeur DHT trop grande")]
    ValueTooLarge,
    /// Bucket inexistant (`refresh_bucket` -> `"no such bucket"`).
    #[error("no such bucket")]
    NoSuchBucket,
    /// Erreur filaire.
    #[error(transparent)]
    Ipv8(#[from] Ipv8Error),
}

/// Type de requete en attente (`Request.msg_type` Python).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PendingKind {
    /// `ping`.
    Ping,
    /// `store`.
    Store,
    /// `find`.
    Find,
    /// `store-peer`.
    StorePeer,
    /// `connect-peer`.
    ConnectPeer,
}

/// Reponse acheminee vers l'appelant en attente.
enum PendingResult {
    /// Pong / store-response recus.
    Ack,
    /// Find-response : `{"values": …}` ou `{"nodes": …}`.
    Find {
        /// Valeurs si non vide.
        values: Vec<Vec<u8>>,
        /// Noeuds sinon.
        nodes: Vec<Arc<Node>>,
    },
    /// Connect-peer-response.
    Nodes(Vec<Arc<Node>>),
}

/// `Request` pyipv8 : entree du request-cache.
struct Pending {
    /// Noeud contacte (metriques).
    node: Arc<Node>,
    /// Instant d'envoi (rtt).
    start: Instant,
    /// Parametres (ex. `force_nodes` pour find, `key` pour store-peer).
    params: PendingParams,
    /// Canal de completion.
    tx: oneshot::Sender<PendingResult>,
}

/// Parametres associes a une requete (`Request.params` Python).
enum PendingParams {
    /// Rien.
    None,
    /// `store-peer` : cle sous laquelle on se fait stocker.
    Key([u8; 20]),
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Encodage base64 standard (RFC 4648) pour `str(node)` des tokens —
/// `base64.b64encode(mid)` Python.
fn b64encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0] as u32,
            *chunk.get(1).unwrap_or(&0) as u32,
            *chunk.get(2).unwrap_or(&0) as u32,
        ];
        let n = (b[0] << 16) | (b[1] << 8) | b[2];
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `str(node)` Python : `Peer<ip:port, b64(mid)>` — entre dans le
/// calcul des tokens (`sha1(str + secret)`).
fn node_repr(node: &Node) -> String {
    let (ip, port) = address_ip_port(&node.address());
    format!("Peer<{ip}:{port}, {}>", b64encode(&node.mid))
}

/// Nom d'interface de classe d'adresse (`FAST_ADDR_TO_INTERFACE`
/// pyipv8 : `UDPv4Address -> "UDPIPv4"`, etc. ; `guess_interface`
/// rend "DNS" pour les adresses domaine).
fn iface_name(class: AddrClass) -> &'static str {
    match class {
        AddrClass::V4 => "UDPIPv4",
        AddrClass::V6 => "UDPIPv6",
        AddrClass::Domain => "DNS",
    }
}

/// Stats d'une table de routage (un element `endpoints` Python de
/// `GET /api/ipv8/dht/statistics`).
#[derive(Debug, Clone)]
pub struct DhtTableStats {
    /// Nom d'interface (`UDPIPv4`/`UDPIPv6`/`DNS`).
    pub endpoint: &'static str,
    /// `hexlify(calc_node_id(address, mid))`.
    pub node_id: String,
    /// Total de noeuds connus.
    pub routing_table_size: usize,
    /// Nombre de buckets.
    pub routing_table_buckets: usize,
    /// Cles dans le storage de cette classe.
    pub num_keys_in_store: usize,
}

/// Instantane `dht_endpoint.get_statistics`.
#[derive(Debug, Clone)]
pub struct DhtStats {
    /// `hexlify(my_peer.mid)`.
    pub peer_id: String,
    /// `len(tokens)`.
    pub num_tokens: usize,
    /// Une entree par table de routage.
    pub endpoints: Vec<DhtTableStats>,
    /// `DHTDiscoveryCommunity.store` : `hex(target)` -> nb de pairs.
    pub num_peers_in_store: Vec<(String, usize)>,
    /// `store_for_me`.
    pub num_store_for_me: Vec<(String, usize)>,
}

/// Instantane d'un noeud de bucket (`get_buckets` Python).
#[derive(Debug, Clone)]
pub struct DhtNodeInfo {
    /// Adresse IP.
    pub ip: String,
    /// Port.
    pub port: u16,
    /// `hexlify(peer.mid)`.
    pub mid: String,
    /// `hexlify(peer.id)` (`None` pour les adresses non-DHT).
    pub id: Option<String>,
    /// Compteur d'echecs.
    pub failed: u32,
    /// `last_contact`.
    pub last_contact: f64,
    /// Distance XOR a notre node_id de cette classe. Python rend
    /// `int(distance())` sur 160 bits — hors u128, on rend donc la
    /// decimale exacte en chaine (divergence de type documentee).
    pub distance: String,
}

/// Instantane d'un bucket (`get_buckets` Python).
#[derive(Debug, Clone)]
pub struct DhtBucketInfo {
    /// Nom d'interface de la table.
    pub endpoint: &'static str,
    /// `prefix_id` binaire.
    pub prefix: String,
    /// `last_changed`.
    pub last_changed: f64,
    /// Noeuds du bucket.
    pub peers: Vec<DhtNodeInfo>,
}

/// Valeur stockee (`get_stored_values` Python — apres
/// `post_process_values`).
#[derive(Debug, Clone)]
pub struct DhtStoredValue {
    /// Nom d'interface.
    pub endpoint: &'static str,
    /// Cle (hex).
    pub key: String,
    /// Donnee brute.
    pub data: Vec<u8>,
    /// Cle publique du signataire (valeurs signees).
    pub public_key: Option<Vec<u8>>,
}

/// Compteurs `debug` de `find_values(key, debug=True)` Python.
#[derive(Debug, Default)]
pub struct CrawlDebug {
    /// Union des `nodes_tried` des crawls.
    nodes_tried: HashSet<[u8; 20]>,
    /// Nb total de reponses.
    pub responses: usize,
    /// Reponses contenant des `nodes`.
    pub responses_with_nodes: usize,
    /// Reponses contenant des `values`.
    pub responses_with_values: usize,
}

impl CrawlDebug {
    /// `len(set().union(*[crawl.nodes_tried]))` Python.
    pub fn requests(&self) -> usize {
        self.nodes_tried.len()
    }
}

/// Conversion big-endian 160 bits -> decimale (`int(distance())`
/// Python, qui deborde u128).
fn big_int_dec(bytes: &[u8; 20]) -> String {
    // digits[0] = unites (little-endian en base 10).
    let mut digits = vec![0u8; 1];
    for &b in bytes.iter() {
        let mut carry = u32::from(b);
        for d in digits.iter_mut() {
            carry += u32::from(*d) * 256;
            *d = (carry % 10) as u8;
            carry /= 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    digits.iter().rev().map(|d| (b'0' + d) as char).collect()
}

/// `Crawl` : etat d'une recherche iterative (`_find` Python).
struct Crawl {
    /// Cible recherchee.
    target: [u8; 20],
    /// Noeuds a contacter : `(noeud, noeud_qui_doit_le_puncturer)`.
    nodes_todo: Vec<(Arc<Node>, Option<Arc<Node>>)>,
    /// Noeuds deja contactes (par id).
    nodes_tried: HashSet<[u8; 20]>,
    /// Reponses recues : `(emetteur, (valeurs, noeuds))`.
    responses: CrawlResponses,
    /// `force_nodes`.
    force_nodes: bool,
}

impl Crawl {
    fn new(
        target: [u8; 20],
        routing_table: &RoutingTable,
        force_nodes: bool,
    ) -> Result<Self, DhtError> {
        let nodes_closest = routing_table.closest_nodes(&target, MAX_CRAWL_NODES, None);
        if nodes_closest.is_empty() {
            return Err(DhtError::NoNodes("aucun noeud dans la table de routage"));
        }
        Ok(Self {
            target,
            nodes_todo: nodes_closest.into_iter().map(|n| (n, None)).collect(),
            nodes_tried: HashSet::new(),
            responses: Vec::new(),
            force_nodes,
        })
    }

    /// `add_response` : enregistre la reponse et enrichit `nodes_todo`
    /// (seuls les noeuds meilleurs que le top-4 courant, puncture par
    /// l'emetteur pour les non-premiers — cf. Python).
    fn add_response(&mut self, sender: Arc<Node>, values: Vec<Vec<u8>>, nodes: Vec<Arc<Node>>) {
        self.responses
            .push((sender.clone(), (values, nodes.clone())));
        for (index, node) in nodes.into_iter().enumerate() {
            let Some(nid) = node.id() else { continue };
            if self.nodes_tried.contains(&nid) {
                continue;
            }
            if self.nodes_todo.len() >= 4 {
                let worse = self.nodes_todo[3]
                    .0
                    .id()
                    .map(|top4| distance(&nid, &self.target) > distance(&top4, &self.target))
                    .unwrap_or(true);
                if worse {
                    continue;
                }
            }
            let puncture = if index == 0 {
                None
            } else {
                Some(sender.clone())
            };
            if let Some(t) = self.nodes_todo.iter_mut().find(|(n, _)| n.key == node.key) {
                t.1 = puncture;
                continue;
            }
            self.nodes_todo.push((node, puncture));
            self.nodes_todo.sort_by_key(|(n, _)| {
                n.id()
                    .map(|id| distance(&id, &self.target))
                    .unwrap_or([0xff; 20])
            });
        }
    }

    /// `done`.
    fn done(&self) -> bool {
        self.nodes_tried.len() >= MAX_CRAWL_REQUESTS || self.nodes_todo.is_empty()
    }

    /// `cache_candidate` : noeud le plus proche n'ayant pas rendu de
    /// valeurs (pour y cacher la paire apres le crawl).
    fn cache_candidate(&self) -> Option<Arc<Node>> {
        self.responses
            .iter()
            .filter(|(_, (values, _))| values.is_empty())
            .map(|(sender, _)| sender.clone())
            .min_by_key(|n| {
                n.id()
                    .map(|id| distance(&id, &self.target))
                    .unwrap_or([0xff; 20])
            })
    }

    /// `values` : fusion sans doublons en preservant l'ordre
    /// (`zip_longest` Python : premiere valeur de chaque reponse, puis
    /// la seconde, etc.).
    fn values(&self) -> Vec<Vec<u8>> {
        let lists: Vec<&Vec<Vec<u8>>> = self
            .responses
            .iter()
            .map(|(_, (v, _))| v)
            .filter(|v| !v.is_empty())
            .collect();
        let max_len = lists.iter().map(|v| v.len()).max().unwrap_or(0);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for i in 0..max_len {
            for list in &lists {
                if let Some(v) = list.get(i) {
                    if seen.insert(v.clone()) {
                        out.push(v.clone());
                    }
                }
            }
        }
        out
    }

    /// `nodes` : noeuds deja contactes, tries par distance a la cible.
    fn nodes(&self, rt: &RoutingTable) -> Vec<Arc<Node>> {
        let mut out: Vec<Arc<Node>> = self
            .nodes_tried
            .iter()
            .filter_map(|id| rt.get(id))
            .collect();
        out.sort_by_key(|n| {
            n.id()
                .map(|id| distance(&id, &self.target))
                .unwrap_or([0xff; 20])
        });
        out
    }
}

/// Community DHT IPv8 (DHTCommunity + DHTDiscoveryCommunity).
pub struct DhtCommunity {
    /// Identite locale.
    key: LibNaClSecretKey,
    /// `my_peer.mid`.
    my_mid: [u8; 20],
    /// `my_estimated_wan` (adresse vue par les pairs — mise a jour
    /// quand la discovery l'apprend, `set_my_wan`).
    my_wan: Mutex<UdpAddress>,
    /// `my_estimated_lan`.
    my_lan: Mutex<UdpAddress>,
    /// `network` propre a la community (le Python instancie un
    /// `Network()` dedie, distinct de celui partage).
    network: Network,
    /// `Weak<Self>` pour respawner des taches depuis les handlers
    /// synchrones (equivalent de `register_anonymous_task`).
    weak: std::sync::Weak<Self>,
    /// Endpoint UDP.
    endpoint: Arc<UdpEndpoint>,
    /// Tables de routage par classe d'adresse.
    routing_tables: Mutex<HashMap<AddrClass, RoutingTable>>,
    /// Storages par classe d'adresse.
    storages: Mutex<HashMap<AddrClass, Storage>>,
    /// Request-cache : `(kind, identifier) -> requete`.
    pending: Mutex<HashMap<(PendingKind, u32), Pending>>,
    /// `tokens[node_id] = (timestamp, token)`.
    tokens: TokenMap,
    /// `token_secrets` (2 secrets tournants).
    token_secrets: Mutex<VecDeque<[u8; 16]>>,
    /// `store` de `DHTDiscoveryCommunity` : `target -> noeuds qui
    /// annoncent la cible`.
    store: Mutex<HashMap<[u8; 20], Vec<Arc<Node>>>>,
    /// `store_for_me` : noeuds qui nous hebergent.
    store_for_me: Mutex<HashMap<[u8; 20], Vec<Arc<Node>>>>,
}

impl DhtCommunity {
    /// Cree la community et l'enregistre sur l'endpoint.
    ///
    /// `my_wan`/`my_lan` : estimations d'adresses (pour le calcul
    /// d'`node_id` et le champ `lan_address` des requetes).
    pub async fn new(
        key: LibNaClSecretKey,
        my_wan: UdpAddress,
        my_lan: UdpAddress,
        endpoint: Arc<UdpEndpoint>,
    ) -> Arc<Self> {
        let my_mid = key.public_key().mid();
        let community = Arc::new_cyclic(|weak| Self {
            key,
            my_mid,
            my_wan: Mutex::new(my_wan),
            my_lan: Mutex::new(my_lan),
            weak: weak.clone(),
            network: Network::default(),
            endpoint: endpoint.clone(),
            routing_tables: Mutex::new(HashMap::new()),
            storages: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            token_secrets: Mutex::new(VecDeque::from([rand::random(), rand::random()])),
            store: Mutex::new(HashMap::new()),
            store_for_me: Mutex::new(HashMap::new()),
        });
        let prefix = prefix_of(&DHT_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(prefix, Arc::new(move |src, pkt| c.on_packet(src, pkt)))
            .await;
        community
    }

    /// `my_estimated_wan`.
    pub fn my_wan(&self) -> UdpAddress {
        self.my_wan.lock().unwrap().clone()
    }

    /// `my_estimated_lan`.
    pub fn my_lan(&self) -> UdpAddress {
        self.my_lan.lock().unwrap().clone()
    }

    /// Met a jour l'estimation WAN quand la discovery l'apprend
    /// (`my_estimated_wan` Python est mutable a la reception des
    /// introduction-responses).
    pub fn set_my_wan(&self, wan: UdpAddress) {
        *self.my_wan.lock().unwrap() = wan;
    }

    /// Met a jour l'estimation LAN.
    pub fn set_my_lan(&self, lan: UdpAddress) {
        *self.my_lan.lock().unwrap() = lan;
    }

    /// `global_time` (Lamport approxime par l'horloge, comme l'etape 9).
    fn gtime() -> u64 {
        now_secs() as u64 % 65536
    }

    /// Signe + envoie un payload serialise. `sign_auto` choisit le
    /// layout selon `msg_id` : `dist` pour les intros/punctures
    /// (246/245/249/231…), `ez_send` pur pour les messages DHT.
    async fn send(&self, addr: &UdpAddress, msg_id: u8, body: &[u8]) -> Result<(), Ipv8Error> {
        let pkt = Packet::sign_auto(&DHT_COMMUNITY_ID, msg_id, &self.key, Self::gtime(), body);
        self.endpoint.send_to(addr, &pkt).await
    }

    /// Table de routage de la classe d'adresse du noeud.
    fn with_routing_table<R>(
        &self,
        addr: &UdpAddress,
        f: impl FnOnce(&mut RoutingTable) -> R,
    ) -> R {
        self.with_class_table(AddrClass::of(addr), f)
    }

    /// Acces a la table d'une classe donnee (creee si absente).
    fn with_class_table<R>(&self, class: AddrClass, f: impl FnOnce(&mut RoutingTable) -> R) -> R {
        let mut tables = self.routing_tables.lock().unwrap();
        let my_id = calc_node_id(&self.my_wan(), &self.my_mid).unwrap_or(self.my_mid);
        let rt = tables
            .entry(class)
            .or_insert_with(|| RoutingTable::new(my_id));
        f(rt)
    }

    /// Storage de la classe d'adresse du noeud.
    fn with_storage<R>(&self, addr: &UdpAddress, f: impl FnOnce(&mut Storage) -> R) -> R {
        let mut storages = self.storages.lock().unwrap();
        let st = storages.entry(AddrClass::of(addr)).or_default();
        f(st)
    }

    /// `get_requesting_node` : ajoute le pair a la table et le retourne,
    /// ou `None` s'il est `blocked` (rate-limit).
    fn get_requesting_node(&self, peer: &Peer) -> Option<Arc<Node>> {
        let addr = peer.address.clone()?;
        let node = Node::new(peer.public_key_bin.clone(), peer.mid, addr);
        let blocked = self.with_routing_table(&node.address(), |rt| {
            node.id()
                .and_then(|id| rt.get(&id))
                .map(|n| n.blocked())
                .unwrap_or(false)
        });
        if blocked {
            return None;
        }
        let node = self.with_routing_table(&node.address(), |rt| rt.add(&node).unwrap_or(node));
        node.note_query();
        Some(node)
    }

    /// `on_node_discovered` : ajoute un noeud decouvert a la table et
    /// le ping (mesure RTT) — sauf si notre WAN est inconnu ou si la
    /// source est blacklistee (non implemente : pas de blacklist ici).
    fn on_node_discovered(&self, public_key_bin: Vec<u8>, address: UdpAddress) {
        if self.my_wan().is_unspecified() {
            return;
        }
        let mid = sha1(&public_key_bin);
        let node = Node::new(public_key_bin, mid, address);
        let (existed, added) = self.with_routing_table(&node.address(), |rt| {
            let existed = node.id().map(|id| rt.has(&id)).unwrap_or(false);
            (existed, rt.add(&node))
        });
        if !existed {
            if let Some(n) = added {
                let c = self.weak.upgrade();
                if let Some(c) = c {
                    tokio::spawn(async move {
                        let _ = c.ping(&n).await;
                    });
                }
            }
        }
    }

    /// `ping` : envoie un ping-request et attend la reponse.
    pub async fn ping(self: &Arc<Self>, node: &Arc<Node>) -> Result<Arc<Node>, DhtError> {
        let (id, rx) = self.register(PendingKind::Ping, node.clone(), PendingParams::None);
        let mut w = Writer::new();
        payloads::PingRequest { identifier: id }.pack(&mut w);
        node.note_ping_sent();
        if let Err(e) = self
            .send(&node.address(), msg::PING_REQUEST, &w.into_bytes())
            .await
        {
            self.pending
                .lock()
                .unwrap()
                .remove(&(PendingKind::Ping, id));
            return Err(e.into());
        }
        self.await_pending(PendingKind::Ping, id, node.clone(), rx, REQUEST_TIMEOUT)
            .await?;
        Ok(node.clone())
    }

    /// Enregistre une requete et retourne `(identifier, rx)`.
    fn register(
        &self,
        kind: PendingKind,
        node: Arc<Node>,
        params: PendingParams,
    ) -> (u32, oneshot::Receiver<PendingResult>) {
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().unwrap();
        let id = loop {
            let id = rand::random::<u32>();
            if !pending.contains_key(&(kind, id)) {
                break id;
            }
        };
        pending.insert(
            (kind, id),
            Pending {
                node,
                start: Instant::now(),
                params,
                tx,
            },
        );
        (id, rx)
    }

    /// Attend la reponse ; en cas de timeout, `node.failed += 1`
    /// (`Request.on_timeout` Python).
    async fn await_pending(
        &self,
        kind: PendingKind,
        id: u32,
        node: Arc<Node>,
        rx: oneshot::Receiver<PendingResult>,
        timeout: Duration,
    ) -> Result<PendingResult, DhtError> {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(res)) => {
                let rtt = self
                    .pending
                    .lock()
                    .unwrap()
                    .remove(&(kind, id))
                    .map(|p| p.start.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                // `Request.on_complete` Python.
                node.note_response(rtt.max(0.000_001));
                Ok(res)
            }
            _ => {
                self.pending.lock().unwrap().remove(&(kind, id));
                node.note_failure();
                Err(DhtError::Timeout("requete"))
            }
        }
    }

    /// `token_maintenance` + generation/verification de jetons
    /// (`sha1(str(node) + secret)`, 2 secrets tournants).
    fn generate_token(&self, node: &Node) -> [u8; 20] {
        let secrets = self.token_secrets.lock().unwrap();
        let secret = secrets.back().copied().unwrap_or([0; 16]);
        sha1(&[node_repr(node).as_bytes(), &secret].concat())
    }

    /// `check_token`.
    fn check_token(&self, node: &Node, token: &[u8; 20]) -> bool {
        let secrets = self.token_secrets.lock().unwrap();
        secrets
            .iter()
            .any(|secret| sha1(&[node_repr(node).as_bytes(), secret.as_slice()].concat()) == *token)
    }

    /// `token_maintenance` : rotation des secrets + purge des tokens
    /// expires (a appeler periodiquement, `interval=300` Python).
    pub fn token_maintenance(&self) {
        {
            let mut secrets = self.token_secrets.lock().unwrap();
            if secrets.len() == 2 {
                secrets.pop_front();
            }
            secrets.push_back(rand::random());
        }
        let now = now_secs();
        self.tokens
            .lock()
            .unwrap()
            .retain(|_, (ts, _)| now <= *ts + TOKEN_EXPIRATION_TIME);
    }

    /// `serialize_value`.
    pub fn serialize_value(&self, data: &[u8], sign: bool) -> Vec<u8> {
        payloads::serialize_value(data, sign, &self.key)
    }

    /// `unserialize_value`.
    pub fn unserialize_value(&self, value: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>, u32)> {
        payloads::unserialize_value(value)
    }

    /// `add_value` — insertion directe dans le stockage local
    /// (equivalent du `storage.put` pyipv8 accessible aux peers
    /// REST/tests ; `_store` y recourt pour la part locale).
    pub fn add_value(&self, key: &[u8; 20], value: &[u8], addr: &UdpAddress, max_age: f64) {
        if let Some((_, public_key, version)) = self.unserialize_value(value) {
            let id = public_key
                .as_deref()
                .map(sha1)
                .map(|m| m.to_vec())
                .unwrap_or_else(|| sha1(value).to_vec());
            self.with_storage(addr, |st| st.put(key, value, id, max_age, version));
        }
    }

    /// `store_value` : serialise puis `_store`.
    pub async fn store_value(
        self: &Arc<Self>,
        key: &[u8; 20],
        data: &[u8],
        sign: bool,
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        let value = self.serialize_value(data, sign);
        self.store(key, value).await
    }

    /// `_store`.
    async fn store(
        self: &Arc<Self>,
        key: &[u8; 20],
        value: Vec<u8>,
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        if value.len() > payloads::MAX_ENTRY_SIZE {
            return Err(DhtError::ValueTooLarge);
        }
        let nodes = self.find_nodes(key).await?;
        let nodes = self
            .store_on_nodes(key, vec![value], &nodes[..nodes.len().min(TARGET_NODES)])
            .await?;
        if nodes.is_empty() {
            return Err(DhtError::NoNodes("valeur non stockee"));
        }
        Ok(nodes)
    }

    /// `store_on_nodes` : envoie les store-requests aux noeuds
    /// disposant d'un token valide ; stocke localement si nous sommes
    /// plus proches que le pire des noeuds connus.
    pub async fn store_on_nodes(
        self: &Arc<Self>,
        key: &[u8; 20],
        values: Vec<Vec<u8>>,
        nodes: &[Arc<Node>],
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        if nodes.is_empty() {
            return Err(DhtError::NoNodes("pas de noeud pour stocker"));
        }
        let values = &values[..values.len().min(MAX_VALUES_IN_STORE)];

        // Stockage local si pertinent (cf. Python).
        let largest = nodes
            .iter()
            .filter_map(|n| n.id())
            .map(|id| distance(&id, key))
            .max()
            .unwrap_or([0xff; 20]);
        let my_id = calc_node_id(&self.my_wan(), &self.my_mid).unwrap_or(self.my_mid);
        if nodes.len() < TARGET_NODES || distance(&my_id, key) < largest {
            for v in values.iter().rev() {
                self.add_value(key, v, &nodes[0].address(), MAX_ENTRY_AGE);
            }
        }

        let now = now_secs();
        let mut futs = Vec::new();
        for node in nodes {
            let token = self
                .tokens
                .lock()
                .unwrap()
                .get(&node.id().unwrap_or_default())
                .and_then(|(ts, t)| (now < ts + TOKEN_EXPIRATION_TIME).then_some(*t));
            let Some(token) = token else {
                tracing::debug!("pas de store-request : token absent");
                continue;
            };
            let c = self.clone();
            let node = node.clone();
            let vals = values.to_vec();
            let key = *key;
            futs.push(tokio::spawn(async move {
                let (id, rx) = c.register(PendingKind::Store, node.clone(), PendingParams::None);
                let mut w = Writer::new();
                payloads::StoreRequest {
                    identifier: id,
                    token,
                    target: key,
                    values: vals,
                }
                .pack(&mut w);
                c.send(&node.address(), msg::STORE_REQUEST, &w.into_bytes())
                    .await?;
                c.await_pending(PendingKind::Store, id, node.clone(), rx, REQUEST_TIMEOUT)
                    .await?;
                Ok::<Arc<Node>, DhtError>(node)
            }));
        }
        if futs.is_empty() {
            return Err(DhtError::NoNodes("valeur non stockee"));
        }
        let mut stored = Vec::new();
        for f in futs {
            if let Ok(Ok(n)) = f.await {
                stored.push(n);
            }
        }
        if stored.is_empty() {
            return Err(DhtError::NoNodes("valeur non stockee"));
        }
        Ok(stored)
    }

    /// `_send_find_request`.
    async fn send_find_request(
        self: &Arc<Self>,
        node: &Arc<Node>,
        target: &[u8; 20],
        force_nodes: bool,
        offset: u32,
    ) -> Result<PendingResult, DhtError> {
        let (id, rx) = self.register(PendingKind::Find, node.clone(), PendingParams::None);
        let mut w = Writer::new();
        payloads::FindRequest {
            identifier: id,
            lan_address: self.my_lan(),
            target: *target,
            offset,
            force_nodes,
        }
        .pack(&mut w)?;
        if let Err(e) = self
            .send(&node.address(), msg::FIND_REQUEST, &w.into_bytes())
            .await
        {
            self.pending
                .lock()
                .unwrap()
                .remove(&(PendingKind::Find, id));
            return Err(e.into());
        }
        self.await_pending(PendingKind::Find, id, node.clone(), rx, FIND_TIMEOUT)
            .await
    }

    /// `_contact_node` (une tache du crawl).
    async fn contact_node(
        self: Arc<Self>,
        node: Arc<Node>,
        puncture_node: Option<Arc<Node>>,
        target: [u8; 20],
        force_nodes: bool,
        offset: u32,
    ) -> Option<(Arc<Node>, Vec<Vec<u8>>, Vec<Arc<Node>>)> {
        if let Some(pn) = puncture_node {
            let nid = node.id().unwrap_or(node.mid);
            let _ = self
                .clone()
                .send_find_request(&pn, &nid, force_nodes, 0)
                .await;
        }
        match self
            .clone()
            .send_find_request(&node, &target, force_nodes, offset)
            .await
        {
            Ok(PendingResult::Find { values, nodes }) => {
                self.with_routing_table(&node.address(), |rt| {
                    rt.add(&node);
                });
                Some((node, values, nodes))
            }
            _ => None,
        }
    }

    /// `_find` : crawl iteratif — jusqu'a `MAX_CRAWL_TASKS` requetes
    /// simultanees, `MAX_CRAWL_REQUESTS` noeuds contactes.
    async fn find_crawl(self: &Arc<Self>, crawl: &mut Crawl, offset: u32) {
        let mut tasks: tokio::task::JoinSet<ContactResult> = tokio::task::JoinSet::new();
        loop {
            while !crawl.done() && tasks.len() < MAX_CRAWL_TASKS {
                let (node, puncture_node) = crawl.nodes_todo.remove(0);
                if let Some(id) = node.id() {
                    crawl.nodes_tried.insert(id);
                }
                tasks.spawn(self.clone().contact_node(
                    node,
                    puncture_node,
                    crawl.target,
                    crawl.force_nodes,
                    offset,
                ));
            }
            if tasks.is_empty() {
                break;
            }
            if let Some(Ok(Some((sender, values, nodes)))) = tasks.join_next().await {
                crawl.add_response(sender, values, nodes);
            }
        }
    }

    /// `find` : un crawl par table de routage (Python itere sur
    /// `routing_tables.values()` et fusionne). `force_nodes=true` rend
    /// les noeuds visites ; sinon les valeurs post-traitees.
    pub async fn find(
        self: &Arc<Self>,
        target: &[u8; 20],
        force_nodes: bool,
        offset: u32,
    ) -> Result<FindOutcome, DhtError> {
        Ok(self.find_inner(target, force_nodes, offset).await?.0)
    }

    /// `find` + accumulation des compteurs `debug` (`debug=True` du
    /// `dht_endpoint` : `nodes_tried`/`responses` par crawl).
    async fn find_inner(
        self: &Arc<Self>,
        target: &[u8; 20],
        force_nodes: bool,
        offset: u32,
    ) -> Result<(FindOutcome, CrawlDebug), DhtError> {
        let classes: Vec<AddrClass> = self
            .routing_tables
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        let mut all_nodes = Vec::new();
        let mut all_values = Vec::new();
        let mut debug = CrawlDebug::default();
        for class in classes {
            // Construction du crawl : lecture seule de la table.
            let mut crawl = {
                let mut tables = self.routing_tables.lock().unwrap();
                let rt = match tables.get_mut(&class) {
                    Some(rt) => rt,
                    None => continue,
                };
                // `Crawl(target, rt)` Python leve `DHTError` a la
                // construction si la table n'a aucun noeud a crawler —
                // `find` propagate immediatement (pas de fusion
                // partielle) pour les modes noeuds ET valeurs.
                Crawl::new(*target, rt, force_nodes)?
            };
            self.find_crawl(&mut crawl, offset).await;
            debug.nodes_tried.extend(crawl.nodes_tried.iter().copied());
            debug.responses += crawl.responses.len();
            debug.responses_with_nodes += crawl
                .responses
                .iter()
                .filter(|(_, (_, nodes))| !nodes.is_empty())
                .count();
            debug.responses_with_values += crawl
                .responses
                .iter()
                .filter(|(_, (values, _))| !values.is_empty())
                .count();

            if force_nodes {
                let nodes = self.with_class_table(class, |rt| crawl.nodes(rt));
                all_nodes.extend(nodes);
            } else {
                let values = crawl.values();
                let candidate = crawl.cache_candidate();
                if let (Some(node), true) = (candidate, !values.is_empty()) {
                    let _ = self
                        .clone()
                        .store_on_nodes(target, values.clone(), &[node])
                        .await;
                }
                all_values.extend(values);
            }
        }
        let outcome = if force_nodes {
            FindOutcome::Nodes(all_nodes)
        } else {
            FindOutcome::Values(self.post_process_values(all_values))
        };
        Ok((outcome, debug))
    }

    /// `find_values(key, debug=True)` : valeurs post-traitees +
    /// compteurs de crawl (`dht_endpoint.get_values`).
    pub async fn find_values_debug(
        self: &Arc<Self>,
        target: &[u8; 20],
        offset: u32,
    ) -> Result<(Vec<DhtValue>, CrawlDebug), DhtError> {
        match self.find_inner(target, false, offset).await? {
            (FindOutcome::Values(v), debug) => Ok((v, debug)),
            (FindOutcome::Nodes(_), _) => unreachable!(),
        }
    }

    /// `refresh_bucket` du `dht_endpoint` : `find_values` sur un id
    /// genere dans le prefixe, pour chaque table de routage.
    /// `Err(NoSuchBucket)` si aucun bucket ne couvre ce prefixe ;
    /// `Err(e)` si tous les rafraichissements ont echoue (le Python
    /// renvoie alors `{"success": false, "error": e}` en 200).
    pub async fn refresh_bucket(self: &Arc<Self>, prefix: &str) -> Result<(), DhtError> {
        let targets: Vec<[u8; 20]> = {
            let tables = self.routing_tables.lock().unwrap();
            tables
                .values()
                .flat_map(|rt| rt.buckets())
                .filter(|b| b.prefix_id == prefix)
                .map(|b| b.generate_id())
                .collect()
        };
        if targets.is_empty() {
            return Err(DhtError::NoSuchBucket);
        }
        let mut ok = false;
        let mut last_err = None;
        for t in targets {
            match self.find_values(&t, 0).await {
                Ok(_) => ok = true,
                Err(e) => last_err = Some(e),
            }
        }
        if !ok {
            return Err(last_err.unwrap_or(DhtError::NoSuchBucket));
        }
        Ok(())
    }

    /// `dht_endpoint.get_statistics` : instantane des stats.
    pub fn stats_snapshot(&self) -> DhtStats {
        let my_wan = self.my_wan();
        let endpoints = {
            let tables = self.routing_tables.lock().unwrap();
            let storages = self.storages.lock().unwrap();
            tables
                .iter()
                .map(|(class, rt)| {
                    let buckets: Vec<_> = rt.buckets().collect();
                    DhtTableStats {
                        endpoint: iface_name(*class),
                        node_id: calc_node_id(&my_wan, &self.my_mid)
                            .map(hex::encode)
                            .unwrap_or_default(),
                        routing_table_size: buckets.iter().map(|b| b.nodes.len()).sum(),
                        routing_table_buckets: buckets.len(),
                        num_keys_in_store: storages.get(class).map(|s| s.len()).unwrap_or(0),
                    }
                })
                .collect()
        };
        let to_hex_map = |m: &HashMap<[u8; 20], Vec<Arc<Node>>>| -> Vec<(String, usize)> {
            m.iter().map(|(k, v)| (hex::encode(k), v.len())).collect()
        };
        DhtStats {
            peer_id: hex::encode(self.my_mid),
            num_tokens: self.tokens.lock().unwrap().len(),
            endpoints,
            num_peers_in_store: to_hex_map(&self.store.lock().unwrap()),
            num_store_for_me: to_hex_map(&self.store_for_me.lock().unwrap()),
        }
    }

    /// `dht_endpoint.get_buckets` : instantane des buckets.
    pub fn buckets_snapshot(&self) -> Vec<DhtBucketInfo> {
        let my_wan = self.my_wan();
        let tables = self.routing_tables.lock().unwrap();
        tables
            .iter()
            .flat_map(|(class, rt)| {
                let my_id = calc_node_id(&my_wan, &self.my_mid).unwrap_or(self.my_mid);
                rt.buckets()
                    .map(|b| DhtBucketInfo {
                        endpoint: iface_name(*class),
                        prefix: b.prefix_id.clone(),
                        last_changed: b.last_changed,
                        peers: b
                            .nodes
                            .values()
                            .map(|n| {
                                let (ip, port) = address_ip_port(&n.address());
                                DhtNodeInfo {
                                    ip,
                                    port,
                                    mid: hex::encode(n.mid),
                                    id: n.id().map(hex::encode),
                                    failed: n.failed(),
                                    last_contact: n.last_contact(),
                                    distance: n
                                        .id()
                                        .map(|id| big_int_dec(&distance(&id, &my_id)))
                                        .unwrap_or_default(),
                                }
                            })
                            .collect(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// `dht_endpoint.get_stored_values` : valeurs post-traitees
    /// stockees localement, par classe d'adresse.
    pub fn stored_values(&self) -> Vec<DhtStoredValue> {
        let storages = self.storages.lock().unwrap();
        let mut out = Vec::new();
        for (class, st) in storages.iter() {
            for (key, raw) in st.items_snapshot() {
                for (data, public_key) in self.post_process_values(raw) {
                    out.push(DhtStoredValue {
                        endpoint: iface_name(*class),
                        key: hex::encode(&key),
                        data,
                        public_key,
                    });
                }
            }
        }
        out
    }

    /// `post_process_values` : deserialise, deduplique ; pour les
    /// valeurs signees ne garde que la plus haute version par cle
    /// publique.
    fn post_process_values(&self, values: Vec<Vec<u8>>) -> Vec<DhtValue> {
        let mut signed: HashMap<Vec<u8>, (u32, Vec<u8>)> = HashMap::new();
        let mut unsigned: Vec<Vec<u8>> = Vec::new();
        for v in &values {
            if let Some((data, public_key, version)) = self.unserialize_value(v) {
                match public_key {
                    Some(pk) => {
                        let keep = signed
                            .get(&pk)
                            .map(|(ver, _)| version > *ver)
                            .unwrap_or(true);
                        if keep {
                            signed.insert(pk, (version, data));
                        }
                    }
                    None => unsigned.push(data),
                }
            }
        }
        let mut out: Vec<DhtValue> = signed
            .into_iter()
            .map(|(pk, (_, data))| (data, Some(pk)))
            .collect();
        out.extend(unsigned.into_iter().map(|d| (d, None)));
        out
    }

    /// `find_values`.
    pub async fn find_values(
        self: &Arc<Self>,
        target: &[u8; 20],
        offset: u32,
    ) -> Result<Vec<DhtValue>, DhtError> {
        match self.find(target, false, offset).await? {
            FindOutcome::Values(v) => Ok(v),
            FindOutcome::Nodes(_) => unreachable!(),
        }
    }

    /// `find_nodes`.
    pub async fn find_nodes(
        self: &Arc<Self>,
        target: &[u8; 20],
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        match self.find(target, true, 0).await? {
            FindOutcome::Nodes(n) => Ok(n),
            FindOutcome::Values(_) => unreachable!(),
        }
    }

    // ---- DHTDiscoveryCommunity (msgs 7-10) ----

    /// `store_peer` : enregistre notre pair aupres des noeuds proches
    /// de notre `mid` (a appeler periodiquement, `interval=30` Python).
    pub async fn store_peer(self: &Arc<Self>) -> Result<Vec<Arc<Node>>, DhtError> {
        let key = self.my_mid;
        if self
            .store_for_me
            .lock()
            .unwrap()
            .get(&key)
            .map(|v| v.len())
            .unwrap_or(0)
            >= TARGET_NODES / 2
        {
            return Ok(Vec::new());
        }
        let nodes = self.find_nodes(&key).await.unwrap_or_default();
        let existing: Vec<Vec<u8>> = self
            .store_for_me
            .lock()
            .unwrap()
            .get(&key)
            .map(|v| v.iter().map(|n| n.key.clone()).collect())
            .unwrap_or_default();
        let fresh: Vec<Arc<Node>> = nodes
            .iter()
            .filter(|n| !existing.contains(&n.key))
            .take(TARGET_NODES)
            .map(|n| Node::new(n.key.clone(), n.mid, n.address()))
            .collect();
        self.send_store_peer_request(&key, fresh).await
    }

    /// `send_store_peer_request`.
    async fn send_store_peer_request(
        self: &Arc<Self>,
        key: &[u8; 20],
        nodes: Vec<Arc<Node>>,
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        if nodes.is_empty() {
            return Err(DhtError::NoNodes("pas de noeud pour store-peer"));
        }
        let mut futs = Vec::new();
        for node in nodes {
            let token = self
                .tokens
                .lock()
                .unwrap()
                .get(&node.id().unwrap_or_default())
                .map(|(_, t)| *t);
            let Some(token) = token else { continue };
            let c = self.clone();
            let key = *key;
            futs.push(tokio::spawn(async move {
                let (id, rx) = c.register(
                    PendingKind::StorePeer,
                    node.clone(),
                    PendingParams::Key(key),
                );
                let mut w = Writer::new();
                payloads::StorePeerRequest {
                    identifier: id,
                    token,
                    target: key,
                }
                .pack(&mut w);
                c.send(&node.address(), msg::STORE_PEER_REQUEST, &w.into_bytes())
                    .await?;
                c.await_pending(
                    PendingKind::StorePeer,
                    id,
                    node.clone(),
                    rx,
                    REQUEST_TIMEOUT,
                )
                .await?;
                Ok::<Arc<Node>, DhtError>(node)
            }));
        }
        if futs.is_empty() {
            return Err(DhtError::NoNodes("pair non stocke"));
        }
        let mut out = Vec::new();
        for f in futs {
            if let Ok(Ok(n)) = f.await {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// `connect_peer` : resout un mid en adresses (sans lookup DHT si
    /// deja connu ou joignable directement).
    pub async fn connect_peer(
        self: &Arc<Self>,
        mid: &[u8; 20],
        peer: Option<Peer>,
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        if let Some(nodes) = self.store.lock().unwrap().get(mid) {
            if !nodes.is_empty() {
                return Ok(nodes.clone());
            }
        }
        if let Some(p) = peer {
            if let Some(addr) = p.address.clone() {
                let node = Node::new(p.public_key_bin.clone(), p.mid, addr);
                if self.ping(&node).await.is_ok() {
                    return Ok(vec![node]);
                }
            }
        }
        let nodes = self.find_nodes(mid).await?;
        self.send_connect_peer_request(mid, &nodes[..nodes.len().min(TARGET_NODES)])
            .await
    }

    /// `send_connect_peer_request`.
    async fn send_connect_peer_request(
        self: &Arc<Self>,
        key: &[u8; 20],
        nodes: &[Arc<Node>],
    ) -> Result<Vec<Arc<Node>>, DhtError> {
        // `"No nodes found for connecting to peer"` Python.
        if nodes.is_empty() {
            return Err(DhtError::NoNodes("pas de noeud pour connect-peer"));
        }
        let mut futs = Vec::new();
        for node in nodes {
            let c = self.clone();
            let node = Node::new(node.key.clone(), node.mid, node.address());
            let key = *key;
            futs.push(tokio::spawn(async move {
                let (id, rx) =
                    c.register(PendingKind::ConnectPeer, node.clone(), PendingParams::None);
                let mut w = Writer::new();
                payloads::ConnectPeerRequest {
                    identifier: id,
                    lan_address: c.my_lan(),
                    target: key,
                }
                .pack(&mut w)?;
                c.send(&node.address(), msg::CONNECT_PEER_REQUEST, &w.into_bytes())
                    .await?;
                match c
                    .await_pending(
                        PendingKind::ConnectPeer,
                        id,
                        node.clone(),
                        rx,
                        REQUEST_TIMEOUT,
                    )
                    .await?
                {
                    PendingResult::Nodes(nodes) => Ok(nodes),
                    _ => Err(DhtError::Timeout("connect-peer")),
                }
            }));
        }
        let mut out = Vec::new();
        for f in futs {
            if let Ok(Ok(nodes)) = f.await {
                for n in nodes {
                    if !out.iter().any(|m: &Arc<Node>| m.key == n.key) {
                        out.push(n);
                    }
                }
            }
        }
        if out.is_empty() {
            return Err(DhtError::NoNodes("echec de connexion au pair"));
        }
        Ok(out)
    }

    // ---- Handlers de messages ----

    /// `walk_to` Python : envoie une `introduction-request` sous le
    /// prefixe DHT — point d'entree du bootstrap de la table.
    pub async fn walk_to(&self, addr: &UdpAddress) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        // `IntroductionRequestPayload` ancien format (IPv4) — les
        // bootstrap Tribler sont IPv4.
        crate::payloads::IntroductionRequest {
            destination_address: addr.clone(),
            source_lan_address: self.my_lan(),
            source_wan_address: self.my_wan(),
            advice: false,
            supports_new_style: true,
            connection_type: crate::payloads::ConnectionType::Unknown,
            identifier: rand::random::<u16>(),
            extra_bytes: Vec::new(),
        }
        .pack(&mut w)?;
        self.send(
            addr,
            crate::payloads::msg::INTRODUCTION_REQUEST,
            &w.into_bytes(),
        )
        .await
    }

    /// `on_packet` : dispatch par `msg_id` (comme `decode_map` Python).
    fn on_packet(&self, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        let src_addr = UdpAddress::from(src);

        // Messages non signes : puncture-request (250/232). Jamais de
        // verification de pair (pas de cle publique).
        if !pkt.signed {
            return self.on_puncture_request(src_addr, &pkt);
        }

        let Some(peer) = Peer::new(pkt.public_key_bin.clone(), Some(src_addr.clone())) else {
            return Ok(());
        };
        let mut r = Reader::new(&pkt.payload);
        match pkt.msg_id {
            msg::PING_REQUEST => {
                let p = payloads::PingRequest::unpack(&mut r)?;
                if self.get_requesting_node(&peer).is_none() {
                    return Ok(());
                }
                self.reply(src_addr, msg::PING_RESPONSE, {
                    let mut w = Writer::new();
                    payloads::PingResponse {
                        identifier: p.identifier,
                    }
                    .pack(&mut w);
                    w.into_bytes()
                });
            }
            msg::PING_RESPONSE => {
                let p = payloads::PingResponse::unpack(&mut r)?;
                self.complete(&peer, PendingKind::Ping, p.identifier, PendingResult::Ack);
            }
            msg::STORE_REQUEST => {
                let p = payloads::StoreRequest::unpack(&mut r)?;
                let Some(node) = self.get_requesting_node(&peer) else {
                    return Ok(());
                };
                if p.values.iter().any(|v| v.len() > payloads::MAX_ENTRY_SIZE)
                    || p.values.len() > MAX_VALUES_IN_STORE
                {
                    return Ok(());
                }
                if !self.check_token(&node, &p.token) {
                    return Ok(());
                }
                // Age reduit selon le nombre de noeuds plus proches
                // (protection anti-surcaching, cf. Python).
                let my_id = calc_node_id(&self.my_wan(), &self.my_mid).unwrap_or(self.my_mid);
                let num_closer = self.with_routing_table(&src_addr, |rt| {
                    rt.closest_nodes(&p.target, 20, None)
                        .iter()
                        .filter(|n| {
                            n.id()
                                .map(|id| distance(&id, &p.target) < distance(&my_id, &p.target))
                                .unwrap_or(false)
                        })
                        .count()
                });
                let shift = num_closer.saturating_sub(TARGET_NODES - 1) as u32;
                let max_age = MAX_ENTRY_AGE / 2f64.powi(shift as i32);
                for v in &p.values {
                    self.add_value(&p.target, v, &src_addr, max_age);
                }
                self.reply(src_addr, msg::STORE_RESPONSE, {
                    let mut w = Writer::new();
                    payloads::StoreResponse {
                        identifier: p.identifier,
                    }
                    .pack(&mut w);
                    w.into_bytes()
                });
            }
            msg::STORE_RESPONSE => {
                let p = payloads::StoreResponse::unpack(&mut r)?;
                self.complete(&peer, PendingKind::Store, p.identifier, PendingResult::Ack);
            }
            msg::FIND_REQUEST => {
                let p = payloads::FindRequest::unpack(&mut r)?;
                let Some(node) = self.get_requesting_node(&peer) else {
                    return Ok(());
                };
                let values = if p.force_nodes {
                    Vec::new()
                } else {
                    self.with_storage(&src_addr, |st| {
                        st.get(&p.target, p.offset as usize, Some(MAX_VALUES_IN_FIND))
                    })
                };
                let mut nodes = Vec::new();
                if p.force_nodes || values.is_empty() {
                    nodes = self.with_routing_table(&src_addr, |rt| {
                        rt.closest_nodes(&p.target, MAX_NODES_IN_FIND, Some(&node))
                    });
                    if let Some(first) = nodes.first() {
                        let pkt =
                            self.make_puncture_request(&p.lan_address, &src_addr, p.identifier);
                        let ep = self.endpoint.clone();
                        let addr = first.address();
                        tokio::spawn(async move {
                            let _ = ep.send_to(&addr, &pkt).await;
                        });
                    }
                }
                let token = self.generate_token(&node);
                self.reply_result(src_addr, msg::FIND_RESPONSE, {
                    let mut w = Writer::new();
                    let node_list: Vec<(UdpAddress, Vec<u8>)> =
                        nodes.iter().map(|n| (n.address(), n.key.clone())).collect();
                    let _ = payloads::FindResponse {
                        identifier: p.identifier,
                        token,
                        values,
                        nodes: node_list,
                    }
                    .pack(&mut w);
                    w.into_bytes()
                });
            }
            msg::FIND_RESPONSE => {
                let p = payloads::FindResponse::unpack(&mut r)?;
                // Token enregistre pour un futur store-request, clef
                // par l'id du noeud contacte (`cache.node.id` Python —
                // le noeud de la requete, pas le pair decode).
                if let Some(pend) = self
                    .pending
                    .lock()
                    .unwrap()
                    .get(&(PendingKind::Find, p.identifier))
                    .map(|pend| pend.node.clone())
                {
                    if let Some(id) = pend.id() {
                        self.tokens
                            .lock()
                            .unwrap()
                            .insert(id, (now_secs(), p.token));
                    }
                }
                let nodes: Vec<Arc<Node>> = p
                    .nodes
                    .iter()
                    .map(|(addr, key)| Node::new(key.clone(), sha1(key), addr.clone()))
                    .collect();
                self.complete(
                    &peer,
                    PendingKind::Find,
                    p.identifier,
                    PendingResult::Find {
                        values: p.values,
                        nodes,
                    },
                );
            }
            msg::STORE_PEER_REQUEST => {
                let p = payloads::StorePeerRequest::unpack(&mut r)?;
                let node = Node::new(peer.public_key_bin.clone(), peer.mid, src_addr.clone());
                node.note_query();
                if !self.check_token(&node, &p.token) || p.target != peer.mid {
                    return Ok(());
                }
                let mut store = self.store.lock().unwrap();
                let list = store.entry(p.target).or_default();
                if !list.iter().any(|n| n.key == node.key) {
                    list.push(node);
                }
                drop(store);
                self.reply(src_addr, msg::STORE_PEER_RESPONSE, {
                    let mut w = Writer::new();
                    payloads::StorePeerResponse {
                        identifier: p.identifier,
                    }
                    .pack(&mut w);
                    w.into_bytes()
                });
            }
            msg::STORE_PEER_RESPONSE => {
                let p = payloads::StorePeerResponse::unpack(&mut r)?;
                // Memorise le noeud comme "nous hebergeant" avant de
                // completer la requete.
                let entry = self
                    .pending
                    .lock()
                    .unwrap()
                    .get(&(PendingKind::StorePeer, p.identifier))
                    .map(|pend| {
                        (
                            pend.node.clone(),
                            match &pend.params {
                                PendingParams::Key(k) => *k,
                                _ => [0; 20],
                            },
                        )
                    });
                if let Some((node, key)) = entry {
                    let mut sfm = self.store_for_me.lock().unwrap();
                    let list = sfm.entry(key).or_default();
                    if !list.iter().any(|n| n.key == node.key) {
                        list.push(node);
                    }
                }
                self.complete(
                    &peer,
                    PendingKind::StorePeer,
                    p.identifier,
                    PendingResult::Ack,
                );
            }
            msg::CONNECT_PEER_REQUEST => {
                let p = payloads::ConnectPeerRequest::unpack(&mut r)?;
                let nodes: Vec<Arc<Node>> = self
                    .store
                    .lock()
                    .unwrap()
                    .get(&p.target)
                    .map(|v| v[..v.len().min(MAX_NODES_IN_FIND)].to_vec())
                    .unwrap_or_default();
                // Demande aux noeuds stockes de puncturer le demandeur.
                for node in &nodes {
                    let pkt = self.make_puncture_request(&p.lan_address, &src_addr, p.identifier);
                    let ep = self.endpoint.clone();
                    let addr = node.address();
                    tokio::spawn(async move {
                        let _ = ep.send_to(&addr, &pkt).await;
                    });
                }
                let node_list: Vec<(UdpAddress, Vec<u8>)> =
                    nodes.iter().map(|n| (n.address(), n.key.clone())).collect();
                self.reply_result(src_addr, msg::CONNECT_PEER_RESPONSE, {
                    let mut w = Writer::new();
                    let _ = payloads::ConnectPeerResponse {
                        identifier: p.identifier,
                        nodes: node_list,
                    }
                    .pack(&mut w);
                    w.into_bytes()
                });
            }
            msg::CONNECT_PEER_RESPONSE => {
                let p = payloads::ConnectPeerResponse::unpack(&mut r)?;
                let nodes: Vec<Arc<Node>> = p
                    .nodes
                    .iter()
                    .map(|(addr, key)| Node::new(key.clone(), sha1(key), addr.clone()))
                    .collect();
                self.complete(
                    &peer,
                    PendingKind::ConnectPeer,
                    p.identifier,
                    PendingResult::Nodes(nodes),
                );
            }
            crate::payloads::msg::PUNCTURE | crate::payloads::msg::NEW_PUNCTURE => {
                // `on_puncture` Python : no-op (le trou NAT est ouvert
                // par la reception meme).
            }
            crate::payloads::msg::INTRODUCTION_REQUEST => {
                let p = crate::payloads::IntroductionRequest::unpack(&mut r)?;
                self.on_node_discovered(peer.public_key_bin.clone(), src_addr.clone());
                // Reponse minimale (pas d'introduction d'un tiers ici).
                self.reply_result(src_addr, crate::payloads::msg::INTRODUCTION_RESPONSE, {
                    let mut w = Writer::new();
                    let _ =
                        crate::payloads::IntroductionResponse {
                            destination_address: p.source_wan_address,
                            source_lan_address: self.my_lan(),
                            source_wan_address: self.my_wan(),
                            lan_introduction_address: UdpAddress::Ipv4(
                                std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0),
                            ),
                            wan_introduction_address: UdpAddress::Ipv4(
                                std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0),
                            ),
                            connection_type: crate::payloads::ConnectionType::Unknown,
                            supports_new_style: true,
                            intro_supports_new_style: false,
                            peer_limit_reached: false,
                            identifier: p.identifier,
                            extra_bytes: Vec::new(),
                        }
                        .pack(&mut w);
                    w.into_bytes()
                });
            }
            crate::payloads::msg::INTRODUCTION_RESPONSE => {
                let _p = crate::payloads::IntroductionResponse::unpack(&mut r)?;
                self.on_node_discovered(peer.public_key_bin.clone(), src_addr.clone());
            }
            _ => {
                tracing::trace!(msg_id = pkt.msg_id, "message DHT inconnu");
            }
        }
        Ok(())
    }

    /// Reponse signee spawnnee (envoi asynchrone depuis le handler
    /// synchrone).
    fn reply(&self, dst: UdpAddress, msg_id: u8, body: Vec<u8>) {
        let ep = self.endpoint.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            // `sign_auto` : `dist` uniquement pour les
            // intros/punctures (`DIST_MSG_IDS`), `ez_send` pur sinon.
            let pkt = Packet::sign_auto(
                &DHT_COMMUNITY_ID,
                msg_id,
                &key,
                DhtCommunity::gtime(),
                &body,
            );
            let _ = ep.send_to(&dst, &pkt).await;
        });
    }

    /// Idem `reply` (nom distinct pour lisibilite sur les reponses
    /// structurees).
    fn reply_result(&self, dst: UdpAddress, msg_id: u8, body: Vec<u8>) {
        self.reply(dst, msg_id, body);
    }

    /// `create_puncture_request` Python (msg 250 ancien / 232 nouveau,
    /// **non signe** — `sig=False`).
    fn make_puncture_request(
        &self,
        lan_walker: &UdpAddress,
        wan_walker: &UdpAddress,
        identifier: u32,
    ) -> Vec<u8> {
        let mut w = Writer::new();
        if matches!(lan_walker, UdpAddress::Ipv4(_)) && matches!(wan_walker, UdpAddress::Ipv4(_)) {
            let _ = w.ipv4(lan_walker);
            let _ = w.ipv4(wan_walker);
            w.u16(identifier as u16);
            Packet::pack_unsigned(
                &DHT_COMMUNITY_ID,
                crate::payloads::msg::PUNCTURE_REQUEST,
                Self::gtime(),
                &w.into_bytes(),
            )
        } else {
            let _ = w.ip_address(lan_walker);
            let _ = w.ip_address(wan_walker);
            w.u16(identifier as u16);
            Packet::pack_unsigned(
                &DHT_COMMUNITY_ID,
                crate::payloads::msg::NEW_PUNCTURE_REQUEST,
                Self::gtime(),
                &w.into_bytes(),
            )
        }
    }

    /// `on_puncture_request` : envoie un puncture (signe, msg 249/231)
    /// vers `wan_walker` (ou `lan_walker` si meme IP WAN que nous).
    fn on_puncture_request(&self, _src: UdpAddress, pkt: &Packet) -> Result<(), Ipv8Error> {
        let mut r = Reader::new(&pkt.payload);
        let (lan_walker, wan_walker, identifier, new_style) = match pkt.msg_id {
            crate::payloads::msg::PUNCTURE_REQUEST => {
                let p = PunctureRequestPayload::unpack(&mut r)?;
                (
                    p.lan_walker_address,
                    p.wan_walker_address,
                    p.identifier,
                    false,
                )
            }
            crate::payloads::msg::NEW_PUNCTURE_REQUEST => {
                let p = crate::payloads::NewPunctureRequestPayload::unpack(&mut r)?;
                (
                    p.lan_walker_address,
                    p.wan_walker_address,
                    p.identifier,
                    true,
                )
            }
            _ => return Ok(()),
        };
        // `target` (destinataire du puncture) = lan_walker si le WAN du
        // demandeur partage notre IP WAN ; sinon wan_walker. Le payload
        // contient toujours (my_lan, wan_walker) — cf. `on_puncture_request`.
        let target = if address_ip_port(&wan_walker).0 == address_ip_port(&self.my_wan()).0 {
            lan_walker
        } else {
            wan_walker.clone()
        };
        let ep = self.endpoint.clone();
        let key = self.key.clone();
        let my_lan = self.my_lan();
        tokio::spawn(async move {
            let mut w = Writer::new();
            let msg_id = if new_style {
                let _ = w.ip_address(&my_lan);
                let _ = w.ip_address(&wan_walker);
                crate::payloads::msg::NEW_PUNCTURE
            } else {
                let _ = w.ipv4(&my_lan);
                let _ = w.ipv4(&wan_walker);
                crate::payloads::msg::PUNCTURE
            };
            w.u16(identifier);
            let pkt = Packet::sign(
                &DHT_COMMUNITY_ID,
                msg_id,
                &key,
                DhtCommunity::gtime(),
                &w.into_bytes(),
            );
            let _ = ep.send_to(&target, &pkt).await;
        });
        Ok(())
    }

    /// Complete une requete en attente si l'identifiant correspond.
    fn complete(&self, peer: &Peer, kind: PendingKind, id: u32, res: PendingResult) {
        let pend = self.pending.lock().unwrap().remove(&(kind, id));
        match pend {
            Some(p) => {
                p.node.note_response(p.start.elapsed().as_secs_f64());
                let _ = p.tx.send(res);
            }
            None => {
                tracing::debug!(
                    ?kind,
                    from = %format!("{:?}", peer.address),
                    "reponse DHT sans requete connue"
                );
            }
        }
    }

    // ---- Maintenance ----

    /// `PingChurn.take_step` + `ping_all` : purge des noeuds BAD,
    /// reinsertion des pairs connus, ping periodique, purge des
    /// `store`/`store_for_me`.
    pub fn step(self: &Arc<Self>) {
        let now = now_secs();
        // Purge des noeuds BAD + sync Network + ping periodique
        // (`PingChurn.take_step`).
        let mut to_ping = Vec::new();
        {
            let mut tables = self.routing_tables.lock().unwrap();
            for rt in tables.values_mut() {
                for node in rt.remove_bad_nodes() {
                    self.network.remove_peer_key(&node.key);
                }
            }
            for rt in tables.values() {
                for bucket in rt.buckets() {
                    for node in bucket.nodes.values() {
                        if !self.network.contains_key(&node.key) {
                            if let Some(p) = Peer::new(node.key.clone(), Some(node.address())) {
                                self.network.add_verified(p.clone());
                                self.network
                                    .discover_service(&p.public_key_bin, DHT_COMMUNITY_ID);
                            }
                        }
                        if node.last_ping_sent() + PING_INTERVAL <= now {
                            to_ping.push(node.clone());
                        }
                    }
                }
            }
        }
        for node in to_ping {
            let c = self.clone();
            tokio::spawn(async move {
                let _ = c.ping(&node).await;
            });
        }

        // `ping_all` (DHTDiscoveryCommunity).
        {
            let mut sfm = self.store_for_me.lock().unwrap();
            for nodes in sfm.values_mut() {
                nodes.retain(|n| n.status() != NODE_STATUS_BAD);
                for n in nodes.iter() {
                    if n.last_ping_sent() + PING_INTERVAL <= now {
                        let c = self.clone();
                        let n = n.clone();
                        tokio::spawn(async move {
                            let _ = c.ping(&n).await;
                        });
                    }
                }
            }
        }
        {
            let mut store = self.store.lock().unwrap();
            for nodes in store.values_mut() {
                nodes.retain(|n| now <= n.last_query() + 60.0);
            }
            store.retain(|_, v| !v.is_empty());
        }
    }

    /// `node_maintenance` : rafraichit les buckets inactifs > 15 min.
    pub async fn node_maintenance(self: &Arc<Self>) {
        let now = now_secs();
        let refresh: Vec<[u8; 20]> = {
            let tables = self.routing_tables.lock().unwrap();
            tables
                .values()
                .flat_map(|rt| rt.buckets())
                .filter(|b| now - b.last_changed > BUCKET_REFRESH_AGE)
                .map(|b| (b.prefix_id.clone(), b.generate_id()))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(_, id)| id)
                .collect()
        };
        // Un find_values par prefixe suffit (cf. commentaire Python).
        let mut seen = HashSet::new();
        {
            let tables = self.routing_tables.lock().unwrap();
            for rt in tables.values() {
                for b in rt.buckets() {
                    if now - b.last_changed > BUCKET_REFRESH_AGE {
                        seen.insert(b.prefix_id.clone());
                    }
                }
            }
        }
        for id in refresh {
            let _ = self.find_values(&id, 0).await;
        }
        // Met a jour last_changed des buckets touches.
        let mut tables = self.routing_tables.lock().unwrap();
        for rt in tables.values_mut() {
            for b in rt.buckets_mut() {
                if seen.contains(&b.prefix_id) {
                    b.last_changed = now;
                }
            }
        }
    }

    /// `value_maintenance` : purge des valeurs expirees.
    pub fn value_maintenance(&self) {
        let mut storages = self.storages.lock().unwrap();
        for st in storages.values_mut() {
            st.clean();
        }
    }

    /// Nombre de noeuds dans les tables de routage.
    pub fn node_count(&self) -> usize {
        self.routing_tables
            .lock()
            .unwrap()
            .values()
            .map(RoutingTable::len)
            .sum()
    }

    /// Acces au `Network` interne (pour les tests/inspection).
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// `OverlaySchema` : instantane REST de la community
    /// (`GET /api/ipv8/overlays`). `Network` propre → `is_isolated`
    /// vrai comme `DHTDiscoveryCommunity` pyipv8.
    pub fn overlay_info(&self) -> crate::overlays::OverlayInfo {
        use crate::overlays::{
            dht_msg_name, overlay_peer, OverlayInfo, OverlayStrategy, DEFAULT_MAX_PEERS,
        };
        OverlayInfo {
            community_id: DHT_COMMUNITY_ID,
            my_peer_hex: hex::encode(self.key.public_key().to_bin()),
            global_time: Self::gtime(),
            peers: self
                .network
                .peers_for_service(&DHT_COMMUNITY_ID)
                .iter()
                .map(overlay_peer)
                .collect(),
            overlay_name: "DHTDiscoveryCommunity",
            max_peers: DEFAULT_MAX_PEERS,
            is_isolated: true,
            my_estimated_wan: self.my_wan(),
            my_estimated_lan: self.my_lan(),
            // `BaseLauncher.get_walk_strategies` Tribler.
            strategies: vec![OverlayStrategy {
                name: "RandomWalk",
                target_peers: 20,
            }],
            decode: dht_msg_name,
        }
    }
}

/// Resultat de `find` : noeuds visites ou valeurs post-traitees.
pub enum FindOutcome {
    /// `force_nodes=true` : liste des noeuds contactes.
    Nodes(Vec<Arc<Node>>),
    /// Sinon : valeurs `(data, pubkey|None)`.
    Values(Vec<DhtValue>),
}
