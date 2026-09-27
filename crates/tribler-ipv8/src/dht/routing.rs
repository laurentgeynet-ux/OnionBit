//! Routage DHT IPv8 (equivalent de `dht/routing.py`) : `Node`,
//! `Bucket`, `RoutingTable` (trie binaire de prefixes 160 bits),
//! `calc_node_id` et `distance`.
//!
//! Fidelite protocolaire : `calc_node_id` reproduit exactement
//! `crc32(ip_masquee)[:3] + mid[:17]` du Python. Attention : le code
//! Python appelle `binascii.crc32` — c'est bien le CRC-32 **IEEE/zlib**
//! (poly 0xEDB88320, reflechi), pas CRC32C malgre le commentaire
//! "crc32c" du docstring.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;

use crate::address::UdpAddress;
use crate::error::Ipv8Error;

/// Intervalle de la fenetre de rate-limit par noeud (secondes).
/// `NODE_LIMIT_INTERVAL` Python.
pub const NODE_LIMIT_INTERVAL: f64 = 5.0;
/// Nombre max de requetes par noeud dans l'intervalle.
/// `NODE_LIMIT_QUERIES` Python.
pub const NODE_LIMIT_QUERIES: usize = 10;
/// `NODE_STATUS_GOOD` : repondu recemment (BEP-5).
pub const NODE_STATUS_GOOD: u8 = 2;
/// `NODE_STATUS_UNKNOWN`.
pub const NODE_STATUS_UNKNOWN: u8 = 1;
/// `NODE_STATUS_BAD` : deux echecs consecutifs.
pub const NODE_STATUS_BAD: u8 = 0;
/// Taille max d'un bucket (`MAX_BUCKET_SIZE`).
pub const MAX_BUCKET_SIZE: usize = 8;
/// Fenetre "reponse recente" du statut GOOD (15 min, BEP-5).
const GOOD_NODE_WINDOW_SECS: f64 = 15.0 * 60.0;

/// CRC-32 IEEE 802.3 (zlib `crc32`/`binascii.crc32`), poly reflechi
/// 0xEDB88320. Table generee statiquement — le volume traite (4-8
/// octets par appel) ne justifie pas de dependance externe.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `calc_node_id` Python : `crc32(ip & masque)[:3] + mid[:17]`
/// (20 octets). Masque IPv4 `0x030f3fff`, IPv6 `0x0103070f1f3f7fff`
/// (8 premiers octets), inspire de la section 21.2.2 de BEP-42.
///
/// Retourne `None` pour les adresses `Domain` : le Python appelle
/// `inet_aton` sur le nom d'hote, ce qui echoue — un noeud sans IP
/// numerique n'entre donc jamais dans la table de routage.
pub fn calc_node_id(address: &UdpAddress, mid: &[u8; 20]) -> Option<[u8; 20]> {
    let masked: Vec<u8> = match address {
        UdpAddress::Ipv4(a) => {
            const MASK4: [u8; 4] = [0x03, 0x0f, 0x3f, 0xff];
            let ip = a.ip().octets();
            (0..4).map(|i| ip[i] & MASK4[i]).collect()
        }
        UdpAddress::Ipv6(a) => {
            const MASK6: [u8; 8] = [0x01, 0x03, 0x07, 0x0f, 0x1f, 0x3f, 0x7f, 0xff];
            let ip = a.ip().octets();
            (0..8).map(|i| ip[i] & MASK6[i]).collect()
        }
        UdpAddress::Domain(..) => return None,
    };
    let crc = crc32(&masked).to_be_bytes();
    let mut id = [0u8; 20];
    id[..3].copy_from_slice(&crc[..3]);
    id[3..].copy_from_slice(&mid[..17]);
    Some(id)
}

/// `distance` Python : XOR des deux ids (entier 160 bits).
/// L'ordre lexicographique sur les octets big-endian est identique a
/// l'ordre numerique de l'entier.
pub fn distance(a: &[u8; 20], b: &[u8; 20]) -> [u8; 20] {
    let mut d = [0u8; 20];
    for i in 0..20 {
        d[i] = a[i] ^ b[i];
    }
    d
}

/// `id_to_binary_string` : les 160 bits sous forme de '0'/'1'
/// (indexation des buckets par prefixe).
fn id_to_binary_string(node_id: &[u8; 20]) -> String {
    let mut s = String::with_capacity(160);
    for b in node_id {
        s.push_str(&format!("{b:08b}"));
    }
    s
}

/// Horloge en secondes flottantes depuis l'epoch (timestamps Python).
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Etat mutable d'un noeud DHT (metriques de requetes).
#[derive(Debug, Default)]
struct NodeState {
    /// Derniere reponse recue (epoch sec).
    last_response: f64,
    /// Timestamps des `NODE_LIMIT_QUERIES` dernieres requetes recues.
    last_queries: VecDeque<f64>,
    /// Dernier ping envoye (epoch sec).
    last_ping_sent: f64,
    /// Echecs consecutifs (timeouts).
    failed: u32,
    /// Dernier RTT mesure (secondes).
    rtt: f64,
}

/// `Node` pyipv8 : pair DHT avec metriques. Partage via `Arc`, etat
/// mutable sous `Mutex`.
#[derive(Debug)]
pub struct Node {
    /// Cle publique binaire (`key_to_bin`).
    pub key: Vec<u8>,
    /// MID = SHA-1 de la cle publique.
    pub mid: [u8; 20],
    /// Adresse (mutable : `bucket.add` rafraichit l'adresse).
    state: Mutex<NodeState>,
    /// Adresse courante du noeud.
    addr: Mutex<UdpAddress>,
}

impl Node {
    /// Cree un noeud a partir d'une cle publique binaire.
    pub fn new(key: Vec<u8>, mid: [u8; 20], address: UdpAddress) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            key,
            mid,
            state: Mutex::new(NodeState::default()),
            addr: Mutex::new(address),
        })
    }

    /// Adresse courante.
    pub fn address(&self) -> UdpAddress {
        self.addr.lock().unwrap().clone()
    }

    /// Met a jour l'adresse (`bucket.add` sur noeud connu).
    pub fn set_address(&self, address: UdpAddress) {
        *self.addr.lock().unwrap() = address;
    }

    /// `node.id` : recalcule a chaque appel (l'adresse peut changer).
    pub fn id(&self) -> Option<[u8; 20]> {
        calc_node_id(&self.addr.lock().unwrap(), &self.mid)
    }

    /// `last_query` : timestamp de la derniere requete recue.
    pub fn last_query(&self) -> f64 {
        self.state
            .lock()
            .unwrap()
            .last_queries
            .back()
            .copied()
            .unwrap_or(0.0)
    }

    /// `last_contact` : max(last_response, last_query).
    pub fn last_contact(&self) -> f64 {
        let s = self.state.lock().unwrap();
        s.last_response
            .max(s.last_queries.back().copied().unwrap_or(0.0))
    }

    /// Enregistre une requete entrante (`last_queries.append`).
    pub fn note_query(&self) {
        let mut s = self.state.lock().unwrap();
        if s.last_queries.len() == NODE_LIMIT_QUERIES {
            s.last_queries.pop_front();
        }
        s.last_queries.push_back(now());
    }

    /// Reponse recue : `last_response`, `failed = 0`, `rtt`.
    pub fn note_response(&self, rtt: f64) {
        let mut s = self.state.lock().unwrap();
        s.last_response = now();
        s.failed = 0;
        s.rtt = rtt;
    }

    /// Timeout de requete : `failed += 1`.
    pub fn note_failure(&self) {
        self.state.lock().unwrap().failed += 1;
    }

    /// Marque l'envoi d'un ping.
    pub fn note_ping_sent(&self) {
        self.state.lock().unwrap().last_ping_sent = now();
    }

    /// `last_ping_sent`.
    pub fn last_ping_sent(&self) -> f64 {
        self.state.lock().unwrap().last_ping_sent
    }

    /// RTT mesure (0 si inconnu).
    pub fn rtt(&self) -> f64 {
        self.state.lock().unwrap().rtt
    }

    /// `blocked` : >= `NODE_LIMIT_QUERIES` requetes dans
    /// `NODE_LIMIT_INTERVAL` secondes.
    pub fn blocked(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.last_queries.len() >= NODE_LIMIT_QUERIES
            && now() - s.last_queries[0] < NODE_LIMIT_INTERVAL
    }

    /// `status` : BAD / GOOD / UNKNOWN (logique BEP-5).
    pub fn status(&self) -> u8 {
        let s = self.state.lock().unwrap();
        if s.failed >= 2 {
            return NODE_STATUS_BAD;
        }
        let t = now();
        let last_query = s.last_queries.back().copied().unwrap_or(0.0);
        if (t - s.last_response) < GOOD_NODE_WINDOW_SECS
            || (s.last_response > 0.0 && (t - last_query) < GOOD_NODE_WINDOW_SECS)
        {
            return NODE_STATUS_GOOD;
        }
        NODE_STATUS_UNKNOWN
    }
}

impl PartialEq for Node {
    /// `__eq__` Python : meme cle publique.
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Node {}

/// `Bucket` : noeuds partageant un prefixe binaire commun.
pub struct Bucket {
    /// Noeuds indexes par id.
    pub nodes: HashMap<[u8; 20], std::sync::Arc<Node>>,
    /// Prefixe binaire ('0'/'1') couvert par le bucket.
    pub prefix_id: String,
    /// Derniere modification (epoch sec).
    pub last_changed: f64,
    /// Capacite (`MAX_BUCKET_SIZE`).
    max_size: usize,
}

impl Bucket {
    fn new(prefix_id: String, max_size: usize) -> Self {
        Self {
            nodes: HashMap::new(),
            prefix_id,
            last_changed: 0.0,
            max_size,
        }
    }

    /// `generate_id` : id aleatoire dans l'espace du prefixe (bits du
    /// prefixe fixes, suffixe tire au hasard).
    pub fn generate_id(&self) -> [u8; 20] {
        let mut id = [0u8; 20];
        let mut bitstr = self.prefix_id.clone();
        for _ in 0..(160 - self.prefix_id.len()) {
            bitstr.push(if rand::random::<bool>() { '1' } else { '0' });
        }
        for (i, ch) in bitstr.chars().enumerate() {
            if ch == '1' {
                id[i / 8] |= 0x80 >> (i % 8);
            }
        }
        id
    }

    /// `owns` : le prefixe couvre-t-il cet id ?
    fn owns(&self, node_id: &[u8; 20]) -> bool {
        id_to_binary_string(node_id).starts_with(&self.prefix_id)
    }

    /// `Bucket.add` : mise a jour d'un noeud existant ou insertion si
    /// de la place (avec eviction BEP-5 d'un noeud BAD ou 2x plus lent).
    fn add(&mut self, node: &std::sync::Arc<Node>) -> bool {
        let Some(id) = node.id() else { return false };
        if !self.owns(&id) {
            return false;
        }
        if let Some(curr) = self.nodes.get(&id) {
            curr.set_address(node.address());
            self.last_changed = now();
            return true;
        }
        if self.nodes.len() >= self.max_size {
            // Evince un noeud BAD, sinon un noeud >= 2x plus lent.
            let mut evict: Option<[u8; 20]> = self
                .nodes
                .values()
                .find(|n| n.status() == NODE_STATUS_BAD)
                .and_then(|n| n.id());
            if evict.is_none() && node.rtt() > 0.0 {
                evict = self
                    .nodes
                    .values()
                    .find(|n| n.rtt() / node.rtt() >= 2.0)
                    .and_then(|n| n.id());
            }
            if let Some(e) = evict {
                self.nodes.remove(&e);
            }
        }
        if self.nodes.len() < self.max_size {
            self.nodes.insert(id, node.clone());
            self.last_changed = now();
            return true;
        }
        false
    }

    /// `Bucket.split` : deux sous-buckets prefixe+'0'/'1' si plein.
    fn split(&self) -> Option<(Bucket, Bucket)> {
        if self.nodes.len() < self.max_size {
            return None;
        }
        let mut b0 = Bucket::new(format!("{}0", self.prefix_id), self.max_size);
        let mut b1 = Bucket::new(format!("{}1", self.prefix_id), self.max_size);
        for node in self.nodes.values() {
            let id = node.id()?;
            if b0.owns(&id) {
                b0.add(node);
            } else if b1.owns(&id) {
                b1.add(node);
            }
        }
        Some((b0, b1))
    }
}

/// `RoutingTable` : arbre binaire de buckets indexe par prefixe.
///
/// Le trie Python (`dht/trie.py`) est represente par une `HashMap`
/// prefixe -> bucket ; les operations `longest_prefix`/`suffixes`
/// s'expriment directement sur les cles.
pub struct RoutingTable {
    /// Notre propre id de noeud.
    pub my_node_id: [u8; 20],
    /// Buckets par prefixe binaire ("" = racine).
    buckets: HashMap<String, Bucket>,
}

impl RoutingTable {
    /// Cree une table avec le bucket racine vide (prefixe "").
    pub fn new(my_node_id: [u8; 20]) -> Self {
        let mut buckets = HashMap::new();
        buckets.insert(String::new(), Bucket::new(String::new(), MAX_BUCKET_SIZE));
        Self {
            my_node_id,
            buckets,
        }
    }

    /// `get_bucket` : bucket du plus long prefixe couvrant `node_id`.
    fn bucket_key_of(&self, node_id: &[u8; 20]) -> String {
        let bits = id_to_binary_string(node_id);
        let mut best = String::new();
        for k in self.buckets.keys() {
            if bits.starts_with(k.as_str()) && k.len() > best.len() {
                best = k.clone();
            }
        }
        best
    }

    /// Iterateur sur tous les buckets (pour maintenance).
    pub fn buckets(&self) -> impl Iterator<Item = &Bucket> {
        self.buckets.values()
    }

    /// Iterateur mutable (mise a jour de `last_changed`).
    pub fn buckets_mut(&mut self) -> impl Iterator<Item = &mut Bucket> {
        self.buckets.values_mut()
    }

    /// `add` : insere le noeud, en splittant le bucket si plein et
    /// seulement s'il contient `my_node_id` (regle Kademlia classique).
    /// Retourne le noeud stocke (existant ou nouveau).
    pub fn add(&mut self, node: &std::sync::Arc<Node>) -> Option<std::sync::Arc<Node>> {
        let id = node.id()?;
        loop {
            let key = self.bucket_key_of(&id);
            let bucket = self.buckets.get_mut(&key)?;
            if bucket.add(node) {
                return bucket.nodes.get(&id).cloned();
            }
            // Echec : bucket plein. Split possible seulement si notre
            // propre id y tombe.
            if bucket.owns(&self.my_node_id) {
                let (b0, b1) = bucket.split()?;
                self.buckets.remove(&key);
                self.buckets.insert(b0.prefix_id.clone(), b0);
                self.buckets.insert(b1.prefix_id.clone(), b1);
                continue;
            }
            return None;
        }
    }

    /// `remove_bad_nodes` : retire les noeuds BAD de tous les buckets.
    pub fn remove_bad_nodes(&mut self) -> Vec<std::sync::Arc<Node>> {
        let mut removed = Vec::new();
        for bucket in self.buckets.values_mut() {
            let bad: Vec<[u8; 20]> = bucket
                .nodes
                .iter()
                .filter(|(_, n)| n.status() == NODE_STATUS_BAD)
                .map(|(id, _)| *id)
                .collect();
            for id in bad {
                if let Some(n) = bucket.nodes.remove(&id) {
                    removed.push(n);
                }
            }
        }
        removed
    }

    /// `has` : id connu ?
    pub fn has(&self, node_id: &[u8; 20]) -> bool {
        self.get(node_id).is_some()
    }

    /// `get` : noeud par id.
    pub fn get(&self, node_id: &[u8; 20]) -> Option<std::sync::Arc<Node>> {
        self.buckets
            .get(&self.bucket_key_of(node_id))?
            .nodes
            .get(node_id)
            .cloned()
    }

    /// Nombre total de noeuds.
    pub fn len(&self) -> usize {
        self.buckets.values().map(|b| b.nodes.len()).sum()
    }

    /// `true` si vide.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Suffixes connus sous `prefix` : pour chaque cle de bucket
    /// `prefix + s`, retourne `s` (inclut "" si `prefix` est un bucket).
    fn suffixes(&self, prefix: &str) -> Vec<String> {
        self.buckets
            .keys()
            .filter(|k| k.starts_with(prefix))
            .map(|k| k[prefix.len()..].to_string())
            .collect()
    }

    /// `closest_nodes` : noeuds les plus proches de `node_id`
    /// (excluant BAD et `exclude_node`), tries par (distance, statut).
    pub fn closest_nodes(
        &self,
        node_id: &[u8; 20],
        max_nodes: usize,
        exclude_node: Option<&Node>,
    ) -> Vec<std::sync::Arc<Node>> {
        let hash_binary = id_to_binary_string(node_id);
        // Plus long prefixe de bucket couvrant la cible.
        let mut prefix = String::new();
        for k in self.buckets.keys() {
            if hash_binary.starts_with(k.as_str()) && k.len() > prefix.len() {
                prefix = k.clone();
            }
        }

        let mut nodes: Vec<std::sync::Arc<Node>> = Vec::new();
        'outer: for i in (0..=prefix.len()).rev() {
            for suffix in self.suffixes(&prefix[..i]) {
                let key = format!("{}{}", &prefix[..i], suffix);
                if let Some(bucket) = self.buckets.get(&key) {
                    for node in bucket.nodes.values() {
                        if node.status() == NODE_STATUS_BAD {
                            continue;
                        }
                        if let Some(ex) = exclude_node {
                            if node.id() == ex.id() {
                                continue;
                            }
                        }
                        if !nodes.iter().any(|n| n.key == node.key) {
                            nodes.push(node.clone());
                        }
                    }
                }
            }
            if nodes.len() > max_nodes {
                break 'outer;
            }
        }
        nodes.sort_by_key(|n| {
            (
                n.id()
                    .map(|id| distance(&id, node_id))
                    .unwrap_or([0xff; 20]),
                n.status(),
            )
        });
        nodes.truncate(max_nodes);
        nodes
    }
}

/// Adresse IP numerique d'une `UdpAddress` (pour `str(node)` des
/// tokens : `Peer<ip:port, b64(mid)>`).
pub fn address_ip_port(address: &UdpAddress) -> (String, u16) {
    match address {
        UdpAddress::Ipv4(a) => (IpAddr::V4(*a.ip()).to_string(), a.port()),
        UdpAddress::Ipv6(a) => (IpAddr::V6(*a.ip()).to_string(), a.port()),
        UdpAddress::Domain(h, p) => (h.clone(), *p),
    }
}

/// Erreur DHT.
pub fn err(msg: &'static str) -> Ipv8Error {
    Ipv8Error::Malformed(msg)
}
