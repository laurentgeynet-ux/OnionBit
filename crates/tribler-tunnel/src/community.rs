//! `TunnelCommunity` : circuits onion routing sur `tribler-ipv8`.
//!
//! Port de `messaging/anonymization/community.py` + `crypto.py`
//! (pyipv8) : creation de circuit (`create`/`created`), extension
//! (`extend`/`extended`), relais de cellules (decrypt FORWARD /
//! encrypt BACKWARD par couche), sortie UDP, `destroy`, `ping`/`pong`.
//! Les cellules ne sont pas des `Packet` IPv8 signes : la community
//! s'enregistre en listener brut (`add_raw_prefix_listener`) et
//! distingue elle-meme cellules (`msg_id == 0`) et paquets signes.
//!
//! Community id : `81ded07332bdc775aa5a46f96de9f8f390bbc9f3`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::seq::SliceRandom;
use rand::RngCore;
use tribler_crypto::ipv8::dh::crypto_box_beforenm;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_crypto::ipv8::session::{
    crypto_auth, crypto_auth_verify, generate_session_keys, Direction, SessionKeys,
};
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::packet::{prefix_of, Packet};
use tribler_ipv8::payloads::{self as ip, Payload};
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::serializer::{Reader, Writer};
use tribler_ipv8::{Ipv8Error, UdpAddress};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::cell::{self, Cell};
use crate::hidden_services::{E2ERequest, LinkRequest};
use crate::payload::{self as tp, msg, Cellable};
use crate::routing::{
    Circuit, Hop, RelayRoute, RoutingObject, Swarm, UnverifiedHop, CIRCUIT_STATE_CLOSING,
    CIRCUIT_STATE_READY, CIRCUIT_TYPE_DATA, PEER_FLAG_EXIT_BACKUP, PEER_FLAG_EXIT_BT,
    PEER_FLAG_EXIT_HTTP, PEER_FLAG_EXIT_IPV8, PEER_FLAG_RELAY,
};
use crate::settings::TunnelSettings;
use crate::TUNNEL_COMMUNITY_ID;

/// Flags de sortie (tous types confondus) pour le partitionnement
/// des candidats `created`/`extended`.
const ANY_EXIT_FLAGS: i32 = PEER_FLAG_EXIT_BT | PEER_FLAG_EXIT_IPV8 | PEER_FLAG_EXIT_HTTP;
/// Capacite du canal `data_rx` (cellules `data` livrees au
/// consommateur — SOCKS5/DHT-over-tunnel).
const DATA_CHANNEL_CAP: usize = 512;
/// `peers_list[:4]` Python : candidats relay/sortie annonces dans
/// `created`/`extended`.
const CANDIDATES_IN_RESPONSE: usize = 4;
/// Capacite du canal broadcast `e2e_ready`.
const E2E_CHANNEL_CAP: usize = 64;
/// Capacite du canal `circuit_removed` (`Notification.circuit_removed`
/// pyipv8 — relaye en topic SSE `tunnel_removed` par `tribler-core`).
const CIRCUIT_REMOVED_CHANNEL_CAP: usize = 64;
/// Capacite du canal de chunks `http-response` par requete en cours.
const HTTP_REQUEST_PARTS_CAP: usize = 64;

/// Evenement "donnee recue sur un circuit" (livre au consommateur —
/// equivalent du dispatch `on_data` vers SOCKS5/services internes).
#[derive(Debug, Clone)]
pub struct CircuitData {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `sock_addr` de la source (dernier saut).
    pub source: SocketAddr,
    /// `dest_address` du payload.
    pub destination: UdpAddress,
    /// `org_address` du payload.
    pub origin: UdpAddress,
    /// Donnees brutes.
    pub data: Vec<u8>,
}

/// Etat mutable de la community.
pub(crate) struct Inner {
    /// `circuits` : circuits dont on est l'initiateur.
    pub(crate) circuits: HashMap<u32, Circuit>,
    /// `relay_from_to` : circuit_id entrant -> route sortante.
    pub(crate) relays: HashMap<u32, RelayRoute>,
    /// `exit_sockets` : circuit_id -> etat de sortie (hop + activation).
    pub(crate) exit_sockets: HashMap<u32, ExitState>,
    /// `CreateRequestCache` : identifiant du create envoye ->
    /// contexte d'extend en attente d'un `created`.
    pub(crate) create_requests: HashMap<u16, CreateRequest>,
    /// `CreatedRequestCache` : circuit_id -> contexte de join en
    /// attente d'un `extend` (conserve apres le premier extend).
    pub(crate) created_requests: HashMap<u32, CreatedRequest>,
    /// `RetryRequestCache` : circuit_id -> contexte du create/extend
    /// en vol (identifier + candidats alternatifs + essais restants).
    pub(crate) retry_requests: HashMap<u32, RetryEntry>,
    /// Flags de service locaux (`settings.peer_flags` : RELAY par
    /// defaut ; 0 = refuser les `create`).
    pub(crate) peer_flags: i32,
    /// `intro_point_for` : `seeder_pk` -> (circuit_id de sortie,
    /// info_hash) — on est le point d'introduction.
    pub(crate) intro_point_for: HashMap<Vec<u8>, (u32, [u8; 20])>,
    /// `pex` : `info_hash` -> store d'introductions (`PexCommunity`
    /// reduite a ses donnees — `pex.rs`).
    pub(crate) pex: HashMap<[u8; 20], crate::pex::PexStore>,
    /// `rendezvous_point_for` : `cookie` -> circuit_id de sortie —
    /// on est le point de rendez-vous.
    pub(crate) rendezvous_point_for: HashMap<[u8; 20], u32>,
    /// `swarms` : hidden swarms rejoints (`join_swarm`).
    pub(crate) swarms: HashMap<[u8; 20], Swarm>,
    /// `IPRequestCache` : identifier -> attente d'`intro-established`.
    pub(crate) ip_requests: HashMap<u16, tokio::sync::oneshot::Sender<()>>,
    /// `RPRequestCache` : identifier -> attente de `rendezvous-
    /// established` (renvoie l'adresse WAN annoncee).
    pub(crate) rp_requests: HashMap<u16, tokio::sync::oneshot::Sender<UdpAddress>>,
    /// `PeersRequestCache` : identifier -> attente de `peers-response`.
    pub(crate) peers_requests:
        HashMap<u16, tokio::sync::oneshot::Sender<Vec<crate::routing::IntroductionPoint>>>,
    /// `E2ERequestCache` : identifier -> contexte e2e en attente d'un
    /// `created-e2e`.
    pub(crate) e2e_requests: HashMap<u16, E2ERequest>,
    /// `LinkRequestCache` : identifier -> attente de `linked-e2e`.
    pub(crate) link_requests: HashMap<u16, LinkRequest>,
    /// Cache `HTTPRequest` (`ipv8-rust-tunnels` `request_cache`) :
    /// identifier u32 -> canal recevant les chunks `http-response`.
    pub(crate) http_requests: HashMap<u32, tokio::sync::mpsc::Sender<tp::HttpResponse>>,
    /// Abonnes par circuit (`subscribe_circuit_data`) — les relais
    /// UDP du hidden seeding captent les donnees de leurs circuits
    /// e2e avant le canal `data_tx` general (SOCKS5).
    pub(crate) data_subscribers: HashMap<u32, tokio::sync::mpsc::Sender<CircuitData>>,
    /// `candidates` Python : `public_key_bin` -> bitmask de flags de
    /// service appris via `ExtraIntroductionPayload` (introductions
    /// signees sur le prefixe tunnel). Alimente `get_candidates`.
    pub(crate) flag_registry: HashMap<Vec<u8>, i32>,
}

/// `TunnelExitSocket` : socket UDP de sortie dediee par circuit —
/// les reponses des destinations externes arrivent hors-prefixe sur
/// cette socket et sont reencapsulees en cellules `data` (BACKWARD).
pub(crate) struct ExitState {
    /// Saut amont (pair precedent + cles de session partagees).
    pub(crate) hop: Hop,
    /// `enabled` : premier octet de donnee vu venant du bon IP.
    pub(crate) enabled: bool,
    /// Socket de sortie dediee.
    pub(crate) socket: Arc<tokio::net::UdpSocket>,
    /// Canal d'arret de la tache de reception : conserve pour son
    /// `Drop` (la tache se termine quand l'entree disparait).
    _stop_tx: tokio::sync::watch::Sender<bool>,
    /// `http_requests` (`exit.rs`) : borne de requetes HTTP
    /// simultanees par circuit de sortie.
    pub(crate) http_permits: Arc<tokio::sync::Semaphore>,
    /// `creation_time` (`RoutingObject` Python) — sert a l'`uptime`
    /// du topic `tunnel_removed`.
    pub(crate) creation_time: std::time::Instant,
    /// `last_activity` Python (`beat_heart` sur trafic sortant/entrant
    /// — `do_remove` `"no activity"`).
    pub(crate) last_activity: std::time::Instant,
    /// `bytes_up + bytes_down` cumules (`max_traffic` Python).
    pub(crate) bytes_total: u64,
}

/// `CreateRequestCache` Python (extend en attente d'un `created`).
pub(crate) struct CreateRequest {
    /// `to_circuit_id` : id du `create` envoye a `to_peer`.
    pub(crate) to_circuit_id: u32,
    /// `from_circuit_id` : id du `extend` recu de `peer`.
    pub(crate) from_circuit_id: u32,
    /// `peer` : pair amont (emetteur de l'extend — cible du `extended`).
    pub(crate) peer: Peer,
    /// `to_peer` : pair aval (destinataire du create).
    pub(crate) to_peer: Peer,
    /// `extend_identifier` : identifier du `extend` recu (repercute
    /// dans le `extended`).
    pub(crate) extend_identifier: u16,
}

/// `RetryRequestCache` Python (`caches.py`) : un create/extend en vol
/// et les candidats a retenter au prochain `next_hop_timeout`.
#[derive(Debug)]
pub(crate) struct RetryEntry {
    /// `packet_identifier` : identifiant du create/extend emis.
    pub(crate) identifier: u16,
    /// Candidats alternatifs (le saut tente en est deja exclu).
    pub(crate) candidates: RetryCandidates,
    /// `max_tries` restant (`circuit_timeout // next_hop_timeout` au
    /// depart, decremente a chaque tentative).
    pub(crate) max_tries: i32,
}

/// Alternatives de saut selon la phase en cours.
#[derive(Debug, Clone)]
pub(crate) enum RetryCandidates {
    /// Premiers sauts alternatifs (`send_initial_create` Python).
    FirstHops(Vec<Peer>),
    /// Cles publiques des candidats d'extension restants
    /// (`send_extend` Python — resolus via `request.candidates` ou
    /// l'annuaire reseau cote dernier hop).
    ExtendKeys(Vec<Vec<u8>>),
}

/// `CreatedRequestCache` Python (join en attente d'un `extend`).
pub(crate) struct CreatedRequest {
    /// Pair amont.
    pub(crate) peer: Peer,
    /// Candidats proposes (`peers_dict` : pubkey_bin -> Peer).
    pub(crate) candidates: HashMap<Vec<u8>, Peer>,
}

/// Champs communs d'une `introduction-request` (formats ancien 246
/// et nouveau 234), decodes pour `on_introduction_request`.
pub(crate) struct IntroRequestParams<'a> {
    /// Identifiant a renvoyer dans la reponse.
    pub(crate) identifier: u16,
    /// LAN annonce du demandeur.
    pub(crate) source_lan: UdpAddress,
    /// WAN annonce du demandeur.
    pub(crate) source_wan: UdpAddress,
    /// Requete au format `ip_address` (msg 234).
    pub(crate) new_style: bool,
    /// `extra_bytes` (portent les `peer_flags` du demandeur).
    pub(crate) extra_bytes: &'a [u8],
}

/// `TunnelCommunity`.
pub struct TunnelCommunity {
    /// `community_id` (prefixe reseau) — `TUNNEL_COMMUNITY_ID` pyipv8
    /// par defaut ; `a3591a6b…d6bc` pour `TriblerTunnelCommunity`
    /// (Tribler >= 7.x).
    pub(crate) community_id: tribler_ipv8::CommunityId,
    /// Identite locale (LibNaCL).
    pub(crate) key: LibNaClSecretKey,
    /// Annuaire reseau partage.
    pub(crate) network: Arc<Network>,
    /// Endpoint UDP partage.
    pub(crate) endpoint: Arc<UdpEndpoint>,
    /// Etat interne.
    pub(crate) inner: Mutex<Inner>,
    /// Compteur `identifier` des requetes (`number` du RequestCache).
    pub(crate) identifier: AtomicU16,
    /// `global_time` (Lamport local pour les paquets signes, ex.
    /// `destroy`).
    pub(crate) global_time: AtomicU64,
    /// Cellules `data` livrees aux consommateurs (broadcast : chaque
    /// proxy SOCKS5 de lane recoit tout et filtre par `return_map` —
    /// un mpsc unique laissait les lanes 2/3 sans retour de donnees).
    pub(crate) data_tx: tokio::sync::broadcast::Sender<CircuitData>,
    /// Canal `e2e_ready` : (circuit_id, info_hash) quand `linked-e2e`
    /// termine la liaison (callback `e2e_callbacks` Python).
    pub(crate) e2e_ready_tx: tokio::sync::broadcast::Sender<(u32, [u8; 20])>,
    /// Version incrementee a chaque mutation des circuits (creation,
    /// hop ajoute — `READY` possible —, destruction). Permet aux
    /// watchdogs de lanes anonymes de reagir a la perte d'un circuit
    /// sans attendre leur intervalle de sondage.
    pub(crate) circuits_changed_tx: tokio::sync::watch::Sender<u64>,
    /// `Notification.circuit_removed` pyipv8 : detail de chaque objet
    /// de routage detruit (circuit, relais, socket de sortie) —
    /// relaye vers le bus `tribler-core` sans dependance inverse.
    pub(crate) circuit_removed_tx: tokio::sync::broadcast::Sender<CircuitRemovedEvent>,
    /// `test_channel` de `ipv8-rust-tunnels` (`socket.rs`) :
    /// `(identifier, octets_cellule)` de chaque `test-response` (cell.
    /// 22) recue — consomme par `run_speedtest`.
    pub(crate) test_tx: tokio::sync::broadcast::Sender<(u32, usize)>,
    /// `DiscoveryCommunity` de la meme stack (injectee via
    /// [`Self::set_discovery`]) : source de `my_estimated_lan/wan`
    /// pour les introductions et punctures (le `my_peer` Python est
    /// partage entre overlays sur le meme endpoint).
    pub(crate) discovery: Mutex<Option<Arc<tribler_ipv8::discovery::DiscoveryCommunity>>>,
    /// `TunnelSettings` Python (`self.settings`) : tous les seuils et
    /// cadences de la community (defauts = valeurs officielles).
    pub settings: TunnelSettings,
}

/// Detail d'un objet de routage detruit (`circuit_removed` pyipv8) —
/// champs du topic SSE `tunnel_removed` cote GUI Tribler.
#[derive(Debug, Clone)]
pub struct CircuitRemovedEvent {
    /// `circuit_id` de l'objet detruit.
    pub circuit_id: u32,
    /// Nom de classe Python (`"Circuit"`, `"RelayRoute"`,
    /// `"TunnelExitSocket"`).
    pub circuit_class: &'static str,
    /// Octets montants cumules.
    pub bytes_up: u64,
    /// Octets descendants cumules.
    pub bytes_down: u64,
    /// Secondes depuis la creation (`uptime` Python).
    pub uptime_secs: f64,
    /// Contexte de destruction (`additional_info` Python).
    pub additional_info: String,
}

/// Instantane d'un circuit (endpoint `/api/ipv8/tunnel/circuits`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CircuitInfo {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// Sauts vises.
    pub goal_hops: usize,
    /// Sauts verifies.
    pub actual_hops: usize,
    /// Type (`CIRCUIT_TYPE_*`).
    #[serde(rename = "type")]
    pub ctype: String,
    /// Etat (`EXTENDING`/`READY`/`CLOSING`).
    pub state: String,
    /// Octets envoyes.
    pub bytes_up: u64,
    /// Octets recus.
    pub bytes_down: u64,
    /// Flags de sortie connus du dernier saut.
    pub exit_flags: i32,
    /// Info-hash associe (swarms), hex.
    pub info_hash: Option<String>,
    /// `verified_hops` : mid hex de chaque saut verifie.
    pub verified_hops: Vec<String>,
    /// `unverified_hop` : mid hex du saut en cours d'ajout (`""`
    /// sinon).
    pub unverified_hop: String,
    /// `creation_time` epoch secondes.
    pub creation_time: u64,
}

/// Instantane d'un relais (`/api/ipv8/tunnel/relays`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelayInfo {
    /// Circuit entrant.
    pub circuit_id_in: u32,
    /// Circuit sortant.
    pub circuit_id_out: u32,
    /// Relais de rendez-vous (hidden services).
    pub rendezvous_relay: bool,
    /// Octets relayes (aller).
    pub bytes_up: u64,
    /// Octets relayes (retour).
    pub bytes_down: u64,
}

/// Instantane d'une socket de sortie (`/api/ipv8/tunnel/exits`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExitInfo {
    /// Circuit servi.
    pub circuit_id: u32,
    /// Sortie active.
    pub enabled: bool,
}

/// Instantane d'un swarm hidden (`/api/ipv8/tunnel/swarms`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SwarmInfo {
    /// Info-hash du swarm (hex).
    pub info_hash: String,
    /// Connexions e2e etablies.
    pub num_connections: usize,
    /// `true` si ce noeud est le seeder du swarm.
    pub seeder: bool,
}

/// Pair tunnel connu (`/api/ipv8/tunnel/peers`) — shape de
/// `get_peers` pyipv8 : `{ip, port, mid, is_key_compatible, flags}`
/// avec `flags` = LISTE des `PEER_FLAG_*` (set Python), pas le
/// bitmask agrege.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelPeerInfo {
    /// Adresse IP du pair.
    pub ip: String,
    /// Port UDP du pair.
    pub port: u16,
    /// `peer.mid` hex (SHA-1 de la cle publique).
    pub mid: String,
    /// Cle compatible avec notre crypto (LibNaCL).
    pub is_key_compatible: bool,
    /// Flags de service annonces, un bit par element.
    pub flags: Vec<i32>,
}

/// `mask -> Vec<flag>` : expansion du bitmask interne en liste de
/// valeurs (`candidates` Python stocke un set d'entiers).
fn flags_to_list(mask: i32) -> Vec<i32> {
    (0..32)
        .filter_map(|b| {
            let flag = 1i32.checked_shl(b)?;
            (mask & flag != 0).then_some(flag)
        })
        .collect()
}

impl TunnelCommunity {
    /// Cree la community et s'enregistre en listener brut sur le
    /// prefixe tunnel. `peer_flags` : flags de service
    /// (`PEER_FLAG_*` ; 0 = ne pas joindre de circuits).
    pub async fn new(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
        peer_flags: i32,
    ) -> Arc<Self> {
        Self::new_with_id(
            key,
            network,
            endpoint,
            TunnelSettings {
                peer_flags,
                ..TunnelSettings::default()
            },
            TUNNEL_COMMUNITY_ID,
        )
        .await
    }

    /// `new` avec un `community_id` explicite (ex. prefixe de
    /// `TriblerTunnelCommunity` pour l'interop Tribler installe) et
    /// les `TunnelSettings` complets (seuils/cadences Python).
    pub async fn new_with_id(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
        settings: TunnelSettings,
        community_id: tribler_ipv8::CommunityId,
    ) -> Arc<Self> {
        let (data_tx, _) = tokio::sync::broadcast::channel(DATA_CHANNEL_CAP);
        let (e2e_ready_tx, _) = tokio::sync::broadcast::channel(E2E_CHANNEL_CAP);
        let (circuits_changed_tx, _) = tokio::sync::watch::channel(0u64);
        let (circuit_removed_tx, _) = tokio::sync::broadcast::channel(CIRCUIT_REMOVED_CHANNEL_CAP);
        let (test_tx, _) =
            tokio::sync::broadcast::channel(crate::speedtest::SPEED_TEST_CHANNEL_CAP);
        let community = Arc::new(Self {
            community_id,
            key,
            network,
            endpoint: endpoint.clone(),
            inner: Mutex::new(Inner {
                circuits: HashMap::new(),
                relays: HashMap::new(),
                exit_sockets: HashMap::new(),
                create_requests: HashMap::new(),
                created_requests: HashMap::new(),
                retry_requests: HashMap::new(),
                peer_flags: settings.peer_flags,
                intro_point_for: HashMap::new(),
                pex: HashMap::new(),
                rendezvous_point_for: HashMap::new(),
                swarms: HashMap::new(),
                ip_requests: HashMap::new(),
                rp_requests: HashMap::new(),
                peers_requests: HashMap::new(),
                e2e_requests: HashMap::new(),
                link_requests: HashMap::new(),
                http_requests: HashMap::new(),
                data_subscribers: HashMap::new(),
                flag_registry: HashMap::new(),
            }),
            identifier: AtomicU16::new(0),
            global_time: AtomicU64::new(0),
            data_tx,
            e2e_ready_tx,
            circuits_changed_tx,
            circuit_removed_tx,
            test_tx,
            discovery: Mutex::new(None),
            settings,
        });
        let weak = Arc::downgrade(&community);
        endpoint
            .add_raw_prefix_listener(
                prefix_of(&community_id),
                Arc::new(move |src, data| {
                    let Some(c) = weak.upgrade() else {
                        return Ok(());
                    };
                    c.on_raw_datagram(src, data);
                    Ok(())
                }),
            )
            .await;
        community
    }

    /// `number` du RequestCache Python (module 2**16).
    pub(crate) fn next_id(&self) -> u16 {
        self.identifier.fetch_add(1, Ordering::Relaxed)
    }

    /// `claim_global_time` pour les paquets signes hors cellule.
    pub(crate) fn claim_global_time(&self) -> u64 {
        self.global_time.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Receveur des donnees de circuit (un seul consommateur — le
    /// premier appel prend le receveur, les suivants obtiennent
    /// `None`).
    /// Flux des cellules `data` entrantes non abonnees par circuit
    /// (`on_data` vers SOCKS5 pyipv8). Multi-abonnes : chaque lane
    /// SOCKS5 s'abonne et ne garde que les `circuit_id` de sa
    /// `return_map`.
    pub fn data_rx(&self) -> tokio::sync::broadcast::Receiver<CircuitData> {
        self.data_tx.subscribe()
    }

    /// Abonne un consommateur aux donnees d'un circuit (relais UDP du
    /// hidden seeding). Les `CircuitData` du circuit vont a ce canal ;
    /// les autres circuits restent sur `data_rx`.
    pub fn subscribe_circuit_data(
        &self,
        circuit_id: u32,
    ) -> tokio::sync::mpsc::Receiver<CircuitData> {
        let (tx, rx) = tokio::sync::mpsc::channel(DATA_CHANNEL_CAP);
        self.inner
            .lock()
            .unwrap()
            .data_subscribers
            .insert(circuit_id, tx);
        rx
    }

    /// Retire l'abonnement de donnees d'un circuit.
    pub fn unsubscribe_circuit_data(&self, circuit_id: u32) {
        self.inner
            .lock()
            .unwrap()
            .data_subscribers
            .remove(&circuit_id);
    }

    /// Nombre de circuits initiates connus.
    pub fn circuit_count(&self) -> usize {
        self.inner.lock().unwrap().circuits.len()
    }

    /// Abonnement aux changements d'etat des circuits : le receveur se
    /// reveille sur chaque creation, hop ajoute (transition `READY`
    /// possible) et destruction. Utilise par le watchdog de sante des
    /// lanes anonymes (`tribler-core`).
    pub fn watch_circuits(&self) -> tokio::sync::watch::Receiver<u64> {
        self.circuits_changed_tx.subscribe()
    }

    /// Signale aux abonnes de [`Self::watch_circuits`] qu'un circuit a
    /// change d'etat.
    pub(crate) fn notify_circuits_changed(&self) {
        self.circuits_changed_tx.send_modify(|v| *v += 1);
    }

    /// Instantane des circuits pour `/api/ipv8/tunnel/circuits`
    /// (`get_circuits` pyipv8 : id, goal_hops, hops, type, state,
    /// bytes).
    pub fn circuits_info(&self) -> Vec<CircuitInfo> {
        self.inner
            .lock()
            .unwrap()
            .circuits
            .values()
            .map(|c| CircuitInfo {
                circuit_id: c.base.circuit_id,
                goal_hops: c.goal_hops,
                actual_hops: c.hops.len(),
                ctype: c.ctype.clone(),
                // Python : `f"{state} ({closing_info})"` si fermeture.
                state: match c.closing_info() {
                    Some(info) => format!("{} ({info})", c.state()),
                    None => c.state().to_string(),
                },
                bytes_up: c.base.bytes_up,
                bytes_down: c.base.bytes_down,
                exit_flags: c.exit_flags,
                info_hash: c.info_hash.map(hex::encode),
                // `hexlify(hop.mid)` — mid = SHA-1 de la cle publique.
                verified_hops: c
                    .hops
                    .iter()
                    .map(|h| hex::encode(tribler_crypto::hash::ipv8_mid(&h.public_key_bin)))
                    .collect(),
                unverified_hop: c
                    .unverified_hop
                    .as_ref()
                    .map(|h| hex::encode(tribler_crypto::hash::ipv8_mid(&h.public_key_bin)))
                    .unwrap_or_default(),
                creation_time: c.base.creation_epoch,
            })
            .collect()
    }

    /// Instantane des relais pour `/api/ipv8/tunnel/relays`
    /// (`get_relays` pyipv8 : cid_in -> cid_out, rendezvous).
    pub fn relays_info(&self) -> Vec<RelayInfo> {
        self.inner
            .lock()
            .unwrap()
            .relays
            .iter()
            .map(|(in_cid, r)| RelayInfo {
                circuit_id_in: *in_cid,
                circuit_id_out: r.base.circuit_id,
                rendezvous_relay: r.rendezvous_relay,
                bytes_up: r.base.bytes_up,
                bytes_down: r.base.bytes_down,
            })
            .collect()
    }

    /// Instantane des sockets de sortie pour `/api/ipv8/tunnel/exits`
    /// (`get_exits` pyipv8 : cid -> enabled).
    pub fn exits_info(&self) -> Vec<ExitInfo> {
        self.inner
            .lock()
            .unwrap()
            .exit_sockets
            .keys()
            .map(|cid| ExitInfo {
                circuit_id: *cid,
                enabled: true,
            })
            .collect()
    }

    /// Instantane des swarms (hidden services) pour
    /// `/api/ipv8/tunnel/swarms` (`get_swarms` pyipv8).
    pub fn swarms_info(&self) -> Vec<SwarmInfo> {
        self.inner
            .lock()
            .unwrap()
            .swarms
            .values()
            .map(|s| SwarmInfo {
                info_hash: hex::encode(s.info_hash),
                num_connections: s.connections.len(),
                seeder: s.seeder_sk.is_some(),
            })
            .collect()
    }

    /// `pex.items()` → `info_hash` + `get_intro_points()`
    /// (`get_pex_peers` pyipv8) : points d'introduction appris puis
    /// nos propres annonces (`intro_points_for` → `my_peer` sur
    /// `my_estimated_wan` — `0.0.0.0:0` sans estimation WAN).
    pub fn pex_intro_points(&self) -> Vec<([u8; 20], Vec<crate::routing::IntroductionPoint>)> {
        let mut inner = self.inner.lock().unwrap();
        let our_key = self.key.public_key().to_bin();
        let our_wan = UdpAddress::unspecified();
        let now = crate::pex::epoch_secs();
        inner
            .pex
            .iter_mut()
            .map(|(ih, store)| (*ih, store.intro_points(&our_key, &our_wan, now)))
            .collect()
    }

    /// Pairs tunnel connus avec leurs flags de service pour
    /// `/api/ipv8/tunnel/peers` (`get_peers` pyipv8 : iteration sur
    /// `candidates` — les pairs sans objet `Peer` resolvable sont
    /// omis, comme un candidat inconnu du `Network`).
    pub fn tunnel_peers_info(&self) -> Vec<TunnelPeerInfo> {
        let registry = self.inner.lock().unwrap().flag_registry.clone();
        registry
            .iter()
            .filter_map(|(pk, mask)| {
                let peer = self.network.get_by_key(pk)?;
                let (ip, port) = tribler_ipv8::overlays::addr_parts(peer.address.as_ref());
                Some(TunnelPeerInfo {
                    ip,
                    port,
                    mid: hex::encode(peer.mid),
                    // `crypto.is_key_compatible` Python : vrai pour les
                    // cles LibNaCL (tous nos pairs).
                    is_key_compatible: pk.starts_with(b"LibNaCLPK:"),
                    flags: flags_to_list(*mask),
                })
            })
            .collect()
    }

    /// Ids des circuits `READY`.
    pub fn ready_circuits(&self) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap()
            .circuits
            .values()
            .filter(|c| c.state() == CIRCUIT_STATE_READY)
            .map(|c| c.base.circuit_id)
            .collect()
    }

    /// Ids des circuits `READY` d'un `ctype` (`CIRCUIT_TYPE_*`).
    pub fn ready_circuits_of_type(&self, ctype: &str) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap()
            .circuits
            .values()
            .filter(|c| c.state() == CIRCUIT_STATE_READY && c.ctype == ctype)
            .map(|c| c.base.circuit_id)
            .collect()
    }

    /// Ids des circuits `READY` de `hops` sauts (`goal_hops`).
    pub fn ready_circuits_of_hops(&self, hops: usize) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap()
            .circuits
            .values()
            .filter(|c| c.state() == CIRCUIT_STATE_READY && c.goal_hops == hops)
            .map(|c| c.base.circuit_id)
            .collect()
    }

    /// Ids des circuits `READY` de `hops` sauts dont le dernier saut
    /// annonce le flag `flag` (`exit_flags`, ex.
    /// `PEER_FLAG_EXIT_HTTP`).
    pub fn ready_circuits_of_hops_flags(&self, hops: usize, flag: i32) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap()
            .circuits
            .values()
            .filter(|c| {
                c.state() == CIRCUIT_STATE_READY && c.goal_hops == hops && c.exit_flags & flag != 0
            })
            .map(|c| c.base.circuit_id)
            .collect()
    }

    /// Enregistre les flags de service du dernier saut (`exit_flags`)
    /// quand ils sont connus (decouverte, annonces).
    pub fn set_circuit_exit_flags(&self, circuit_id: u32, flags: i32) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(c) = inner.circuits.get_mut(&circuit_id) {
            c.exit_flags = flags;
        }
    }

    /// Tente de construire des circuits pour `hops` sauts si le nombre
    /// de circuits prets ou en cours est inferieur a `min_circuits`.
    pub async fn build_circuits_if_needed(
        self: &Arc<Self>,
        hops: usize,
        min_circuits: usize,
    ) -> Result<(), Ipv8Error> {
        if hops == 0 || hops > 3 {
            return Ok(());
        }
        let (ready, pending) = {
            let inner = self.inner.lock().unwrap();
            let ready = inner
                .circuits
                .values()
                .filter(|c| c.state() == CIRCUIT_STATE_READY && c.goal_hops == hops)
                .count();
            let pending = inner
                .circuits
                .values()
                .filter(|c| c.state() != CIRCUIT_STATE_READY && c.goal_hops == hops)
                .count();
            (ready, pending)
        };
        if ready + pending >= min_circuits {
            return Ok(());
        }
        // Choix du premier hop : comme `create_circuit` pyipv8, toute
        // la liste des candidats est transmise a `send_initial_create`
        // — chaque timeout de saut retente sur le candidat suivant au
        // lieu de reconverger toujours vers le meme pair.
        // Pour 1 saut : sorties `EXIT_BT` (aleatoire, `select_exit`).
        // Pour 2 ou 3 sauts : relays/sorties les moins utilises.
        let first_hops = if hops == 1 {
            let mut exits = self.get_candidates(crate::routing::PEER_FLAG_EXIT_BT);
            if exits.is_empty() {
                exits = self.network.peers_for_service(&self.community_id);
            }
            exits.shuffle(&mut rand::thread_rng());
            exits
        } else {
            self.first_hop_candidates(CIRCUIT_TYPE_DATA, None)
        };

        let first_hops = if first_hops.is_empty() {
            self.network.all_verified_peers()
        } else {
            first_hops
        };

        if let Some(peer) = first_hops.first() {
            tracing::info!(hops, peer = ?peer.address, "tentative de creation proactive de circuit");
            self.create_circuit_inner(hops, first_hops, CIRCUIT_TYPE_DATA, None, None)
                .await?;
        }
        Ok(())
    }

    /// `_generate_circuit_id` Python (aleatoire, sans collision avec
    /// circuits/relays/exits connus).
    fn gen_circuit_id(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        loop {
            let id = rand::thread_rng().next_u32();
            if !inner.circuits.contains_key(&id)
                && !inner.relays.contains_key(&id)
                && !inner.exit_sockets.contains_key(&id)
            {
                return id;
            }
        }
    }

    /// `send_cell` : serialise le payload en cellule puis applique
    /// `outgoing_crypto` (chiffrement selon le role local pour le
    /// circuit_id : initiateur -> FORWARD sur tous les hops, sortie ->
    /// BACKWARD sur son hop, relais -> direction de l'autre route).
    /// Retourne le nombre d'octets de la cellule envoyee sur le fil
    /// (le `n` de `rt.send_cell` des tunnels Rust — comptabilise dans
    /// les stats du speed-test).
    pub(crate) async fn send_cell<P: Cellable>(
        &self,
        addr: &UdpAddress,
        p: &P,
    ) -> Result<usize, Ipv8Error> {
        let mut w = Writer::new();
        p.pack(&mut w)?;
        let body = w.into_bytes();
        if body.len() < 4 {
            return Err(Ipv8Error::Malformed("cellable sans circuit_id"));
        }
        let circuit_id = u32::from_be_bytes(body[..4].try_into().unwrap());
        let plaintext = cell::NO_CRYPTO_PACKETS.contains(&P::MSG_ID);
        // `cell.message` Python = `inner_msg_id + payload[4:]` : le
        // `circuit_id` n'apparait qu'en en-tete de cellule et est
        // reinsere par `unwrap` a la reception. On construit la
        // cellule en clair puis `encrypt_cell` chiffre `cell[29..]`
        // par couches.
        let mut relay_early = false;
        let mut crypto: Option<(Direction, Vec<SessionKeys>)> = None;
        // Couche e2e additionnelle (`outgoing_crypto` Python :
        // `hs_session_keys` appliquee AVANT les couches par saut —
        // FORWARD sur RP_SEEDER, BACKWARD sinon).
        let mut hs: Option<(Direction, SessionKeys)> = None;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(circuit) = inner.circuits.get_mut(&circuit_id) {
                // Activite sortante + octets (`increase_bytes_sent`/
                // `beat_heart` Python) — sinon le circuit passe
                // `is_inactive` malgre le trafic et `do_remove` le
                // balaye en `"no activity"`, circuits perpetuellement
                // ouverts/fermes.
                circuit.base.last_outgoing = std::time::Instant::now();
                circuit.base.bytes_up += body.len() as u64;
                relay_early = P::MSG_ID == msg::EXTEND
                    || circuit.relay_early_count < self.settings.max_relay_early;
                if relay_early {
                    circuit.relay_early_count += 1;
                }
                if !plaintext {
                    if let Some(k) = &circuit.hs_session_keys {
                        let dir = if circuit.ctype == crate::routing::CIRCUIT_TYPE_RP_SEEDER {
                            Direction::Forward
                        } else {
                            Direction::Backward
                        };
                        hs = Some((dir, k.clone()));
                    }
                    let keys: Vec<SessionKeys> = circuit
                        .hops
                        .iter()
                        .map(|h| h.session_keys.clone())
                        .collect();
                    crypto = Some((Direction::Forward, keys));
                }
            } else if let Some(exit) = inner.exit_sockets.get(&circuit_id) {
                if !plaintext {
                    crypto = Some((Direction::Backward, vec![exit.hop.session_keys.clone()]));
                }
            } else if let Some(relay) = inner.relays.get(&circuit_id) {
                if !plaintext {
                    if relay.rendezvous_relay {
                        // Point de rendez-vous : reponse vers l'aval —
                        // chiffre BACKWARD avec les cles de la jambe
                        // entrante (`outgoing_crypto` Python).
                        crypto = Some((Direction::Backward, vec![relay.hop.session_keys.clone()]));
                    } else if let Some(other) = inner.relays.get(&relay.base.circuit_id) {
                        // Route de retour : chiffre dans la direction de
                        // l'AUTRE route (`other.direction`, `other.hop`).
                        crypto = Some((other.direction, vec![other.hop.session_keys.clone()]));
                    }
                }
            }
        }

        let mut wire = Cell::to_wire(
            &prefix_of(&self.community_id),
            circuit_id,
            P::MSG_ID,
            &body[4..],
            plaintext,
            relay_early,
        );
        if let Some((dir, mut k)) = hs {
            wire = cell::encrypt_cell(&wire, dir, std::slice::from_mut(&mut k))?;
        }
        if let Some((dir, mut keys)) = crypto {
            wire = cell::encrypt_cell(&wire, dir, &mut keys)?;
        }
        self.endpoint.send_to(addr, &wire).await.map(|_| wire.len())
    }

    /// `perform_http_request` (`ipv8-rust-tunnels` `socks5.rs`) :
    /// envoie une requete HTTP brute en cellule `http-request` sur le
    /// circuit `circuit_id` et recolle les chunks `http-response`
    /// (`part`/`total`) jusqu'a la reponse complete.
    pub async fn perform_http_request(
        &self,
        circuit_id: u32,
        target: &UdpAddress,
        request: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, Ipv8Error> {
        let identifier = rand::thread_rng().next_u32();
        let (tx, mut rx) = tokio::sync::mpsc::channel(HTTP_REQUEST_PARTS_CAP);
        self.inner
            .lock()
            .unwrap()
            .http_requests
            .insert(identifier, tx);
        let result = async {
            let p = tp::HttpRequest {
                circuit_id,
                identifier,
                target: target.clone(),
                request: request.to_vec(),
            };
            let addr = {
                let inner = self.inner.lock().unwrap();
                inner
                    .circuits
                    .get(&circuit_id)
                    .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                    .ok_or(Ipv8Error::Malformed("circuit HTTP inconnu"))?
            };
            self.send_cell(&addr, &p).await?;
            // Le `total` est fige au premier chunk recu (les suivants
            // incoherents sont ignores) et l'assemblage exige la
            // contiguite 0..total — un trou = timeout, jamais une
            // reponse partielle silencieuse.
            let collected =
                tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
                    let mut parts: HashMap<u16, Vec<u8>> = HashMap::new();
                    let mut total: Option<u16> = None;
                    while let Some(chunk) = rx.recv().await {
                        let t = *total.get_or_insert(chunk.total);
                        if chunk.total != t || chunk.part >= t {
                            continue;
                        }
                        parts.insert(chunk.part, chunk.response);
                        if parts.len() >= t as usize {
                            break;
                        }
                    }
                    let t = total.unwrap_or(0);
                    let mut out = Vec::new();
                    for i in 0..t {
                        match parts.get(&i) {
                            Some(d) => out.extend_from_slice(d),
                            None => {
                                return Err(Ipv8Error::Malformed("chunk http-response manquant"))
                            }
                        }
                    }
                    Ok::<Vec<u8>, Ipv8Error>(out)
                })
                .await;
            match collected {
                Ok(r) => r,
                Err(_) => Err(Ipv8Error::Malformed("timeout http-response")),
            }
        }
        .await;
        // Retrait sur TOUS les chemins (succes, timeout, erreur
        // d'envoi) : sinon l'entree `http_requests` fuit.
        self.inner.lock().unwrap().http_requests.remove(&identifier);
        result
    }

    /// `on_http_request` (`socket.rs`) : cote sortie — exige
    /// `PEER_FLAG_EXIT_HTTP`, borne les requetes simultanees par un
    /// semaphore, execute la requete TCP puis renvoie la reponse
    /// decoupee en chunks `HTTP_RESPONSE_CHUNK`.
    fn on_http_request(self: &Arc<Self>, circuit_id: u32, p: tp::HttpRequest) {
        if self.inner.lock().unwrap().peer_flags & PEER_FLAG_EXIT_HTTP == 0 {
            tracing::debug!(circuit_id, "http-request refuse (EXIT_HTTP inactif)");
            return;
        }
        let (permit, addr) = {
            let inner = self.inner.lock().unwrap();
            let Some(exit) = inner.exit_sockets.get(&circuit_id) else {
                tracing::debug!(circuit_id, "http-request refuse (sortie inconnue)");
                return;
            };
            let Some(addr) = exit.hop.address.clone() else {
                return;
            };
            match exit.http_permits.clone().try_acquire_owned() {
                Ok(permit) => (permit, addr),
                Err(_) => {
                    tracing::debug!(circuit_id, "http-request refuse (limite atteinte)");
                    return;
                }
            }
        };
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(crate::http_tunnel::HTTP_TCP_TIMEOUT_MS),
                crate::http_tunnel::send_tcp_request(&p.target, &p.request),
            )
            .await;
            let Ok(Ok(response)) = result else {
                tracing::warn!(circuit_id, "requete TCP de sortie en echec");
                return;
            };
            let total = response
                .len()
                .div_ceil(crate::http_tunnel::HTTP_RESPONSE_CHUNK)
                .max(1) as u16;
            for (index, chunk) in response
                .chunks(crate::http_tunnel::HTTP_RESPONSE_CHUNK)
                .enumerate()
            {
                let part = tp::HttpResponse {
                    circuit_id,
                    identifier: p.identifier,
                    part: index as u16,
                    total,
                    response: chunk.to_vec(),
                };
                if let Err(e) = this.send_cell(&addr, &part).await {
                    tracing::warn!(circuit_id, error = %e, "envoi http-response en echec");
                    return;
                }
            }
            drop(permit);
        });
    }

    /// `on_http_response` (`socket.rs`) : cote demandeur — achemine le
    /// chunk vers le cache `HTTPRequest` par `identifier`.
    fn on_http_response(&self, p: tp::HttpResponse) {
        let tx = {
            let inner = self.inner.lock().unwrap();
            inner.http_requests.get(&p.identifier).cloned()
        };
        match tx {
            Some(tx) => {
                let _ = tx.try_send(p);
            }
            None => tracing::trace!(id = p.identifier, "http-response inattendue"),
        }
    }

    /// `create_circuit` : cree un circuit de `goal_hops` sauts dont le
    /// premier hop est `first_hop`. Retourne le `circuit_id`.
    pub async fn create_circuit(
        self: &Arc<Self>,
        goal_hops: usize,
        first_hop: &Peer,
    ) -> Result<u32, Ipv8Error> {
        self.create_circuit_typed(goal_hops, first_hop, CIRCUIT_TYPE_DATA, None, None)
            .await
    }

    /// `create_circuit` complet : `ctype`, `required_exit` (cle publique
    /// binaire exigee comme DERNIER saut), `info_hash` attache.
    pub async fn create_circuit_typed(
        self: &Arc<Self>,
        goal_hops: usize,
        first_hop: &Peer,
        ctype: &str,
        required_exit: Option<Vec<u8>>,
        info_hash: Option<[u8; 20]>,
    ) -> Result<u32, Ipv8Error> {
        // `required_exit` est le DERNIER saut : pour un circuit a 1
        // saut, c'est donc le premier hop (comme pyipv8).
        let effective_first = if goal_hops == 1 {
            required_exit
                .as_ref()
                .and_then(|pk| self.network.get_by_key(pk))
                .unwrap_or_else(|| first_hop.clone())
        } else {
            first_hop.clone()
        };
        self.create_circuit_inner(
            goal_hops,
            vec![effective_first],
            ctype,
            required_exit,
            info_hash,
        )
        .await
    }

    /// Corps commun de `create_circuit` : cree le `Circuit` puis
    /// `send_initial_create` sur la liste ordonnee de premiers sauts
    /// possibles (les alternates servent au retry de
    /// `spawn_hop_timeout`).
    async fn create_circuit_inner(
        self: &Arc<Self>,
        goal_hops: usize,
        first_hops: Vec<Peer>,
        ctype: &str,
        required_exit: Option<Vec<u8>>,
        info_hash: Option<[u8; 20]>,
    ) -> Result<u32, Ipv8Error> {
        if first_hops.is_empty() {
            return Err(Ipv8Error::Malformed("pas de premier hop disponible"));
        }
        let circuit_id = self.gen_circuit_id();
        {
            let mut inner = self.inner.lock().unwrap();
            let mut circuit = Circuit::new(circuit_id, goal_hops, ctype, info_hash);
            circuit.required_exit = required_exit;
            inner.circuits.insert(circuit_id, circuit);
        }
        self.notify_circuits_changed();
        self.send_initial_create(circuit_id, first_hops, self.settings.max_tries())
            .await?;
        Ok(circuit_id)
    }

    /// `send_initial_create` pyipv8 : tente le premier hop
    /// `peers[0]`, enregistre les alternates dans le
    /// `RetryRequestCache` (`retry_requests`) et envoie le `create`.
    /// Chaque retry regenere DH + identifier comme le Python
    /// (`cache.packet_identifier` neuf a chaque tentative).
    async fn send_initial_create(
        self: &Arc<Self>,
        circuit_id: u32,
        peers: Vec<Peer>,
        max_tries: i32,
    ) -> Result<(), Ipv8Error> {
        let Some(first_hop) = peers.first() else {
            return Err(Ipv8Error::Malformed("pas de premier hop disponible"));
        };
        let addr = first_hop
            .address
            .clone()
            .ok_or(Ipv8Error::Malformed("hop sans adresse"))?;
        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(circuit) = inner.circuits.get_mut(&circuit_id) else {
                return Err(Ipv8Error::Malformed("circuit inconnu"));
            };
            circuit.unverified_hop = Some(UnverifiedHop {
                public_key_bin: first_hop.public_key_bin.clone(),
                address: Some(addr.clone()),
                dh_secret,
                identifier,
            });
            inner.retry_requests.insert(
                circuit_id,
                RetryEntry {
                    identifier,
                    candidates: RetryCandidates::FirstHops(peers[1..].to_vec()),
                    max_tries: max_tries - 1,
                },
            );
        }
        self.notify_circuits_changed();
        let create = tp::Create {
            circuit_id,
            identifier,
            node_public_key: self.key.public_key().to_bin(),
            key: dh_public.to_vec(),
        };
        self.send_cell(&addr, &create).await?;
        self.spawn_hop_timeout(circuit_id, identifier);
        Ok(())
    }

    /// `RetryRequestCache.on_timeout` Python : a l'expiration du
    /// `next_hop_timeout`, retente le saut sur le candidat suivant
    /// (`send_initial_create`/`send_extend` avec les alternates) tant
    /// que `max_tries` n'est pas epuise — sinon detruit le circuit.
    /// Sans ce garde-fou un circuit bloque en `EXTENDING` est compte
    /// comme "en cours" par `build_circuits_if_needed` pour toujours.
    fn spawn_hop_timeout(self: &Arc<Self>, circuit_id: u32, identifier: u16) {
        let this = self.clone();
        let timeout = self.settings.next_hop_timeout;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let retry = {
                let inner = this.inner.lock().unwrap();
                let entry = inner.retry_requests.get(&circuit_id);
                let closing = inner
                    .circuits
                    .get(&circuit_id)
                    .is_some_and(|c| c.state() == CIRCUIT_STATE_CLOSING);
                if closing {
                    None
                } else {
                    entry
                        .filter(|e| e.identifier == identifier)
                        .map(|e| (e.candidates.clone(), e.max_tries))
                }
            };
            let Some((candidates, max_tries)) = retry else {
                return;
            };
            let retried = match candidates {
                RetryCandidates::FirstHops(peers) if !peers.is_empty() && max_tries >= 1 => {
                    tracing::debug!(circuit_id, "retry du create sur un premier saut alternatif");
                    this.send_initial_create(circuit_id, peers, max_tries)
                        .await
                        .is_ok()
                }
                RetryCandidates::ExtendKeys(keys) if !keys.is_empty() && max_tries >= 1 => {
                    tracing::debug!(circuit_id, "retry de l'extend sur un candidat alternatif");
                    this.send_extend(circuit_id, keys, max_tries).await.is_ok()
                }
                _ => false,
            };
            if !retried {
                tracing::debug!(
                    circuit_id,
                    identifier,
                    "timeout du saut suivant, circuit abandonne"
                );
                this.remove_circuit(circuit_id, "timeout du saut suivant")
                    .await;
            }
        });
    }

    /// `send_extend` pyipv8 : choisit le candidat suivant (filtre des
    /// sauts deja employes, de soi-meme et de `required_exit`), pose
    /// l'`unverified_hop`, enregistre les alternates dans le
    /// `RetryRequestCache` et envoie l'`ExtendPayload` au premier saut.
    /// `candidates` = cles publiques proposees par le dernier hop
    /// (`candidates_enc` du `created`/`extended`). Sans candidat,
    /// repli sur un pair `EXIT_BT & RELAY` aleatoire (comme Python) ;
    /// sinon le circuit est detruit ("no candidates to extend").
    async fn send_extend(
        self: &Arc<Self>,
        circuit_id: u32,
        candidates: Vec<Vec<u8>>,
        max_tries: i32,
    ) -> Result<(), Ipv8Error> {
        let zero: UdpAddress = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
        let my_pk = self.key.public_key().to_bin();
        let (become_exit, required_exit, first_hop_addr, exclude) = {
            let inner = self.inner.lock().unwrap();
            let Some(c) = inner.circuits.get(&circuit_id) else {
                return Err(Ipv8Error::Malformed("circuit inconnu"));
            };
            let mut exclude: Vec<Vec<u8>> =
                c.hops.iter().map(|h| h.public_key_bin.clone()).collect();
            exclude.push(my_pk);
            if let Some(pk) = &c.required_exit {
                exclude.push(pk.clone());
            }
            (
                c.goal_hops.saturating_sub(1) == c.hops.len(),
                c.required_exit.clone(),
                c.first_hop()
                    .and_then(|h| h.address.clone())
                    .ok_or(Ipv8Error::Malformed("pas de premier hop"))?,
                exclude,
            )
        };

        // Choix du prochain saut (`send_extend` Python).
        let (extend_pk, node_addr, alternates): (Vec<u8>, UdpAddress, Vec<Vec<u8>>) = if become_exit
        {
            if let Some(pk) = required_exit {
                // `required_exit` impose : pas d'alternates.
                let addr = self
                    .network
                    .get_by_key(&pk)
                    .and_then(|p| p.address)
                    .unwrap_or_else(|| zero.clone());
                (pk, addr, Vec::new())
            } else {
                (Vec::new(), zero.clone(), Vec::new())
            }
        } else {
            let valid: Vec<Vec<u8>> = candidates
                .iter()
                .filter(|k| {
                    !exclude.contains(k)
                        && tribler_crypto::ipv8::keys::LibNaClPublicKey::from_bin(k).is_ok()
                })
                .cloned()
                .collect();
            match valid.split_first() {
                Some((pk, rest)) => (pk.clone(), zero.clone(), rest.to_vec()),
                None => (Vec::new(), zero.clone(), Vec::new()),
            }
        };
        let (extend_pk, node_addr, alternates) = if extend_pk.is_empty() {
            // Plus de candidat : les derniers hops proposent
            // normalement des pairs deja pounces ; a defaut on tente
            // une sortie connue (`get_candidates(EXIT_BT, RELAY)`).
            // `get_candidates(EXIT_BT, RELAY)` Python ; quand le
            // registre de flags est vide (peers appris sans
            // `extra_bytes`), repli sur les pairs du service tunnel —
            // superset permissif, les candidats du `created` restent
            // prioritaires.
            let mut choices: Vec<Peer> = self
                .get_candidates_subset(&[PEER_FLAG_EXIT_BT, PEER_FLAG_RELAY])
                .into_iter()
                .filter(|p| !exclude.contains(&p.public_key_bin))
                .collect();
            if choices.is_empty() {
                choices = self
                    .network
                    .peers_for_service(&self.community_id)
                    .into_iter()
                    .filter(|p| !exclude.contains(&p.public_key_bin))
                    .collect();
            }
            // `thread_rng` n'est pas `Send` — borne a l'expression.
            let picked = {
                let mut rng = rand::thread_rng();
                choices.choose(&mut rng).cloned()
            };
            match picked {
                Some(p) => (
                    p.public_key_bin.clone(),
                    p.address.unwrap_or_else(|| zero.clone()),
                    Vec::new(),
                ),
                None => {
                    self.remove_circuit(circuit_id, "no candidates to extend")
                        .await;
                    return Err(Ipv8Error::Malformed("aucun candidat d'extension"));
                }
            }
        } else {
            (extend_pk, node_addr, alternates)
        };

        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        let hop_addr = self
            .network
            .get_by_key(&extend_pk)
            .and_then(|p| p.address)
            .filter(|a| !a.is_unspecified());
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(c) = inner.circuits.get_mut(&circuit_id) else {
                return Err(Ipv8Error::Malformed("circuit inconnu"));
            };
            c.unverified_hop = Some(UnverifiedHop {
                public_key_bin: extend_pk.clone(),
                address: hop_addr,
                dh_secret,
                identifier,
            });
            inner.retry_requests.insert(
                circuit_id,
                RetryEntry {
                    identifier,
                    candidates: RetryCandidates::ExtendKeys(alternates),
                    max_tries: max_tries - 1,
                },
            );
        }
        let p = tp::Extend {
            circuit_id,
            identifier,
            node_public_key: extend_pk,
            key: dh_public.to_vec(),
            node_addr,
        };
        self.send_cell(&first_hop_addr, &p).await?;
        self.spawn_hop_timeout(circuit_id, identifier);
        Ok(())
    }

    /// Adresse du premier saut d'un circuit (`circuit.hop` Python :
    /// `hops[0]` sinon `unverified_hop`).
    pub(crate) fn circuit_first_hop_addr(&self, circuit_id: u32) -> Option<UdpAddress> {
        let inner = self.inner.lock().unwrap();
        inner.circuits.get(&circuit_id).and_then(|c| {
            c.first_hop()
                .and_then(|h| h.address.clone())
                .or_else(|| c.unverified_hop.as_ref().and_then(|h| h.address.clone()))
        })
    }

    /// `get_candidates(*flags)` pyipv8 — sous-ensemble : le pair doit
    /// porter TOUS les flags demandes
    /// (`set(requested) <= set(flags)`).
    pub fn get_candidates_subset(&self, flags: &[i32]) -> Vec<Peer> {
        let inner = self.inner.lock().unwrap();
        let candidates: Vec<Peer> = self
            .network
            .peers_for_service(&self.community_id)
            .into_iter()
            .filter(|p| {
                inner
                    .flag_registry
                    .get(&p.public_key_bin)
                    .is_some_and(|f| flags.iter().all(|flag| f & flag == *flag))
            })
            .collect();
        self.filter_backup_exits(&inner, candidates, flags)
    }

    /// `TriblerTunnelCommunity.get_candidates` : quand `EXIT_BT` est
    /// demande, depriorise les sorties marquees `EXIT_BACKUP` — repli
    /// sur la liste complete si elles sont toutes backup.
    fn filter_backup_exits(
        &self,
        inner: &Inner,
        candidates: Vec<Peer>,
        requested_flags: &[i32],
    ) -> Vec<Peer> {
        if !requested_flags.contains(&PEER_FLAG_EXIT_BT) {
            return candidates;
        }
        let preferred: Vec<Peer> = candidates
            .iter()
            .filter(|p| {
                inner
                    .flag_registry
                    .get(&p.public_key_bin)
                    .is_none_or(|f| f & PEER_FLAG_EXIT_BACKUP == 0)
            })
            .cloned()
            .collect();
        if preferred.is_empty() {
            candidates
        } else {
            preferred
        }
    }

    /// `select_exit` pyipv8 : pair aleatoire portant `exit_flags`, ou
    /// selon `ctype` (`DATA` → `EXIT_BT` ; `IP_SEEDER` →
    /// `EXIT_BT`|`EXIT_IPV8`|`RELAY` ; sinon `RELAY`|`EXIT_BT`).
    fn select_exit(&self, exit_flags: &[i32], ctype: &str) -> Option<Peer> {
        let candidates = if !exit_flags.is_empty() {
            self.get_candidates_subset(exit_flags)
        } else if ctype == crate::routing::CIRCUIT_TYPE_DATA {
            self.get_candidates(crate::routing::PEER_FLAG_EXIT_BT)
        } else if ctype == crate::routing::CIRCUIT_TYPE_IP_SEEDER {
            let c = self.get_candidates(crate::routing::PEER_FLAG_EXIT_BT);
            if c.is_empty() {
                let c = self.get_candidates(crate::routing::PEER_FLAG_EXIT_IPV8);
                if c.is_empty() {
                    self.get_candidates(crate::routing::PEER_FLAG_RELAY)
                } else {
                    c
                }
            } else {
                c
            }
        } else {
            let c = self.get_candidates(crate::routing::PEER_FLAG_RELAY);
            if c.is_empty() {
                self.get_candidates(crate::routing::PEER_FLAG_EXIT_BT)
            } else {
                c
            }
        };
        use rand::seq::SliceRandom;
        candidates.choose(&mut rand::thread_rng()).cloned()
    }

    /// `create_circuit` pyipv8 complet : selection automatique de la
    /// sortie (`select_exit(exit_flags)`) et du premier hop (sortie
    /// requise pour 1 saut ; sinon saut le moins utilise parmi les
    /// premiers hops existants du meme `ctype` + candidats `RELAY` /
    /// `RELAY&EXIT_BT`). `Ok(None)` = "no available exit-nodes" /
    /// "no first hop available" (le Python retourne `None`).
    pub async fn create_circuit_with_flags(
        self: &Arc<Self>,
        goal_hops: usize,
        ctype: &str,
        exit_flags: &[i32],
    ) -> Result<Option<u32>, Ipv8Error> {
        let mut required_exit = if ctype == crate::routing::CIRCUIT_TYPE_IP_SEEDER {
            self.select_exit(&[], ctype)
        } else if !exit_flags.is_empty() {
            self.select_exit(exit_flags, ctype)
        } else {
            None
        };
        let mut required_key = required_exit.as_ref().map(|p| p.public_key_bin.clone());
        let first_hops = if goal_hops == 1 {
            if required_exit.is_none() {
                required_exit = self.select_exit(exit_flags, ctype);
                required_key = required_exit.as_ref().map(|p| p.public_key_bin.clone());
            }
            match required_exit {
                Some(p) => vec![p],
                // "Could not create circuit, no available exit-nodes".
                None => return Ok(None),
            }
        } else {
            self.first_hop_candidates(ctype, required_key.as_deref())
        };
        if first_hops.is_empty() {
            // "Could not create circuit, no first hop available".
            return Ok(None);
        }
        self.create_circuit_inner(goal_hops, first_hops, ctype, required_key, None)
            .await
            .map(Some)
    }

    /// `possible_first_hops` de `create_circuit` pyipv8 : premiers hops
    /// des circuits de meme `ctype` + candidats `RELAY` +
    /// `RELAY & EXIT_BT`, brasses puis tries par frequence d'usage
    /// ascendante (`Counter.most_common().reverse()`), `required_exit`
    /// exclu. Toute la liste est conservee pour le retry alternatif.
    fn first_hop_candidates(&self, ctype: &str, required_key: Option<&[u8]>) -> Vec<Peer> {
        let mut freq: Vec<(Peer, usize)> = Vec::new();
        let mut push = |p: Peer| match freq
            .iter_mut()
            .find(|(q, _)| q.public_key_bin == p.public_key_bin)
        {
            Some((_, n)) => *n += 1,
            None => freq.push((p, 1)),
        };
        {
            let inner = self.inner.lock().unwrap();
            for c in inner.circuits.values() {
                if c.ctype == ctype {
                    if let Some(h) = c.first_hop() {
                        if let Some(p) = Peer::new(h.public_key_bin.clone(), h.address.clone()) {
                            push(p);
                        }
                    }
                }
            }
        }
        for p in self.get_candidates(crate::routing::PEER_FLAG_RELAY) {
            push(p);
        }
        for p in self.get_candidates_subset(&[
            crate::routing::PEER_FLAG_RELAY,
            crate::routing::PEER_FLAG_EXIT_BT,
        ]) {
            push(p);
        }
        let mut possible: Vec<(Peer, usize)> = freq
            .into_iter()
            .filter(|(p, _)| Some(p.public_key_bin.as_slice()) != required_key)
            .collect();
        possible.shuffle(&mut rand::thread_rng());
        // Tri stable par frequence ascendante : les sauts deja utilises
        // passent en dernier, les egalites restent brassées.
        possible.sort_by_key(|(_, n)| *n);
        possible.into_iter().map(|(p, _)| p).collect()
    }

    /// `await circuit.ready` pyipv8 : attend que le circuit atteigne
    /// `READY`, soit detruit, ou que `timeout` expire (borne —
    /// `next_hop_timeout` des settings).
    pub async fn await_circuit_ready(&self, circuit_id: u32) -> bool {
        let mut rx = self.circuits_changed_tx.subscribe();
        let deadline = std::time::Instant::now() + self.settings.next_hop_timeout;
        loop {
            {
                let inner = self.inner.lock().unwrap();
                match inner.circuits.get(&circuit_id) {
                    // `ready.set_result(None)` cote Python quand le
                    // circuit meurt avant d'etre pret.
                    None => return false,
                    Some(c) if c.ready() => return true,
                    _ => {}
                }
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                return false;
            }
        }
    }

    /// `remove_circuit` pyipv8 (`destroy` non envoye — usage
    /// `speed test finished`) : `close()` puis retrait apres
    /// `remove_tunnel_delay`, avec `circuit_removed`.
    pub async fn remove_circuit(self: &Arc<Self>, circuit_id: u32, additional_info: &str) {
        let ev = {
            let mut inner = self.inner.lock().unwrap();
            inner.retry_requests.remove(&circuit_id);
            // `hidden_services.remove_circuit` : un `RP_DOWNLOADER`
            // retire est detache de son swarm (sans le fermer ici).
            if let Some(c) = inner.circuits.get(&circuit_id) {
                if c.ctype == crate::routing::CIRCUIT_TYPE_RP_DOWNLOADER {
                    if let Some(ih) = c.info_hash {
                        if let Some(s) = inner.swarms.get_mut(&ih) {
                            s.remove_connection(circuit_id);
                        }
                    }
                }
            }
            match inner.circuits.get_mut(&circuit_id) {
                // "Cannot remove unknown circuit" — warning Python.
                None => return,
                Some(c) => {
                    c.close(additional_info);
                    CircuitRemovedEvent {
                        circuit_id,
                        circuit_class: "Circuit",
                        bytes_up: c.base.bytes_up,
                        bytes_down: c.base.bytes_down,
                        uptime_secs: c.base.creation_time.elapsed().as_secs_f64(),
                        additional_info: additional_info.to_string(),
                    }
                }
            }
        };
        self.notify_circuits_changed();
        let this = self.clone();
        let info = additional_info.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(this.settings.remove_tunnel_delay).await;
            if this
                .inner
                .lock()
                .unwrap()
                .circuits
                .remove(&circuit_id)
                .is_some()
            {
                this.notify_circuits_changed();
                this.emit_circuit_removed(ev);
                tracing::debug!(circuit_id, info, "circuit retire");
            }
        });
    }

    /// Point d'entree brut (raw prefix listener) : cellule ou paquet.
    pub fn on_raw_datagram(self: &Arc<Self>, src: SocketAddr, data: &[u8]) {
        let prefix = prefix_of(&self.community_id);
        if cell::is_cell(&prefix, data) {
            if let Err(e) = self.process_cell(src, data) {
                tracing::debug!(error = %e, "cellule rejetee");
            }
            return;
        }
        match Packet::parse(
            data,
            Some(&self.community_id),
            &tribler_ipv8::packet::WIRE_DEFAULT,
        ) {
            Ok(pkt) => {
                if let Err(e) = self.on_packet(src, pkt) {
                    tracing::debug!(error = %e, "paquet tunnel rejete");
                }
            }
            Err(_) => {
                // Paquets e2e NON signes (`ezr_pack(sig=False)` :
                // `prefix + msg_id + payload` sans auth/dist) —
                // `on_packet_from_circuit` avec circuit_id=None.
                self.on_packet_from_circuit(src, data, None);
            }
        }
    }

    /// `on_packet_from_circuit` : dispatch d'un paquet de prefixe
    /// tunnel non signe (`ezr_pack(sig=False)`), recu a nu sur la
    /// socket (`circuit_id=None`) ou a l'interieur d'une cellule
    /// `data` (`circuit_id` du circuit porteur).
    pub(crate) fn on_packet_from_circuit(
        self: &Arc<Self>,
        src: SocketAddr,
        data: &[u8],
        circuit_id: Option<u32>,
    ) {
        let prefix = prefix_of(&self.community_id);
        if data.len() <= prefix.len() + 1 || data[..prefix.len()] != prefix {
            return;
        }
        let msg_id = data[prefix.len()];
        let mut r = Reader::new(&data[prefix.len() + 1..]);
        match msg_id {
            msg::CREATE_E2E => {
                if let Ok(p) = tp::CreateE2E::unpack(&mut r) {
                    self.on_create_e2e(src, p, circuit_id);
                }
            }
            msg::CREATED_E2E => {
                if let Ok(p) = tp::CreatedE2E::unpack(&mut r) {
                    let c = self.clone();
                    tokio::spawn(async move {
                        c.on_created_e2e(p, circuit_id).await;
                    });
                }
            }
            msg::PEERS_REQUEST => {
                if let Ok(p) = tp::PeersRequest::unpack(&mut r) {
                    self.on_peers_request(src, p, circuit_id);
                }
            }
            msg::PEERS_RESPONSE => {
                if let Ok(p) = tp::PeersResponse::unpack(&mut r) {
                    self.on_peers_response(p);
                }
            }
            _ => {
                tracing::trace!(msg_id, "paquet tunnel non signe ignore");
            }
        }
    }

    /// `process_cell` Python : routage relais (sans crypto de
    /// controle), puis `incoming_crypto`, puis checks de flags, puis
    /// dispatch du message interne.
    fn process_cell(self: &Arc<Self>, src: SocketAddr, data: &[u8]) -> Result<(), Ipv8Error> {
        let parsed = Cell::parse(data)?;
        let circuit_id = parsed.circuit_id;
        tracing::trace!(circuit_id, ?src, "cellule recue");
        // Relais d'abord (comme `process_cell` Python : `relay_cell`
        // avant `incoming_crypto`). Le guard est libere avant
        // `relay_cell` (sinon self-deadlock du Mutex).
        let relay = {
            let inner = self.inner.lock().unwrap();
            inner.relays.get(&circuit_id).cloned()
        };
        if let Some(route) = relay {
            return self.relay_cell(src, data, &route);
        }

        // `incoming_crypto`.
        enum Crypto {
            None,
            Exit(SessionKeys),
            Circuit(Vec<SessionKeys>, Option<(Direction, SessionKeys)>),
        }
        let crypto = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(exit) = inner.exit_sockets.get_mut(&circuit_id) {
                exit.last_activity = std::time::Instant::now();
                Crypto::Exit(exit.hop.session_keys.clone())
            } else if let Some(c) = inner.circuits.get_mut(&circuit_id) {
                // `beat_heart` + octets sur TOUTE cellule recue de ce
                // circuit (Python : `circuit.beat_heart()` dans
                // `on_packet`/`on_data`) — sans ca, un circuit DATA
                // qui transfere est tout de meme retire `"no
                // activity"` par `do_remove`.
                c.base.beat_heart();
                c.base.bytes_down += data.len() as u64;
                // Couche e2e (`incoming_crypto` Python : FORWARD sur
                // RP_DOWNLOADER, BACKWARD sinon, APRES les hops).
                let hs = c.hs_session_keys.clone().map(|k| {
                    let dir = if c.ctype == crate::routing::CIRCUIT_TYPE_RP_DOWNLOADER {
                        Direction::Forward
                    } else {
                        Direction::Backward
                    };
                    (dir, k)
                });
                Crypto::Circuit(c.hops.iter().map(|h| h.session_keys.clone()).collect(), hs)
            } else if parsed.plaintext {
                Crypto::None
            } else {
                return Err(Ipv8Error::Malformed(
                    "cellule chiffree pour circuit inconnu",
                ));
            }
        };
        let decrypted = match crypto {
            Crypto::None => data.to_vec(),
            Crypto::Exit(k) => cell::decrypt_cell(data, Direction::Forward, &[k])?,
            Crypto::Circuit(ks, hs) => {
                let d = cell::decrypt_cell(data, Direction::Backward, &ks)?;
                if let Some((dir, k)) = hs {
                    cell::decrypt_cell(&d, dir, &[k])?
                } else {
                    d
                }
            }
        };

        // Checks de flags post-decrypt (comme `process_cell` Python).
        cell::check_cell_flags(&decrypted, self.settings.max_relay_early)?;
        let cell = Cell::parse(&decrypted)?;
        self.on_cell_message(src, &cell)
    }

    /// `relay_cell` Python : transforme et reexpedie la cellule.
    fn relay_cell(
        self: &Arc<Self>,
        _src: SocketAddr,
        data: &[u8],
        route: &RelayRoute,
    ) -> Result<(), Ipv8Error> {
        let next_cid = route.base.circuit_id;
        let direction = route.direction;
        let hop = &route.hop;
        let parsed = Cell::parse(data)?;
        if parsed.plaintext {
            return Err(Ipv8Error::Malformed("cellule en clair relayee"));
        }
        if parsed.relay_early && route.relay_early_count >= self.settings.max_relay_early {
            return Err(Ipv8Error::Malformed("trop de cellules relay_early"));
        }
        let transformed = if route.rendezvous_relay {
            // Point de rendez-vous : decrypt FORWARD sur ce relais puis
            // encrypt BACKWARD sur la route correspondante.
            let other_keys = {
                let inner = self.inner.lock().unwrap();
                inner
                    .relays
                    .get(&next_cid)
                    .map(|r| r.hop.session_keys.clone())
            };
            let Some(other) = other_keys else {
                return Err(Ipv8Error::Malformed("route de rendez-vous inconnue"));
            };
            let dec = cell::decrypt_cell(
                data,
                Direction::Forward,
                std::slice::from_ref(&hop.session_keys),
            )?;
            // Un `link-e2e` retransmis apres liaison arrive encore sur
            // cette route : il est destine au point de rendez-vous, pas
            // a relayer — dispatch local (`on_link_e2e` re-repond
            // `linked-e2e` si la paire est deja liee).
            if let Ok(inner_cell) = Cell::parse(&dec) {
                if inner_cell.inner_msg_id == msg::LINK_E2E {
                    return self.on_cell_message(_src, &inner_cell);
                }
            }
            let mut keys = [other];
            cell::encrypt_cell(&dec, Direction::Backward, &mut keys)?
        } else {
            match direction {
                Direction::Forward => cell::decrypt_cell(
                    data,
                    Direction::Forward,
                    std::slice::from_ref(&hop.session_keys),
                )?,
                // Sens retour : on AJOUTE une couche (encrypt).
                Direction::Backward => {
                    let mut keys = [hop.session_keys.clone()];
                    cell::encrypt_cell(data, Direction::Backward, &mut keys)?
                }
            }
        };
        let forwarded = Cell::swap_circuit_id(&transformed, next_cid);
        if let Some(addr) = hop.address.clone() {
            tracing::trace!(next_cid, ?addr, "cellule relayee");
            {
                let mut inner = self.inner.lock().unwrap();
                // beat_heart + compteur sur la route ENTRANTE (cle de
                // la table = circuit_id recu).
                let incoming_cid = parsed.circuit_id;
                if let Some(r) = inner.relays.get_mut(&incoming_cid) {
                    r.base.beat_heart();
                    r.base.bytes_down += data.len() as u64;
                    r.relay_early_count += 1;
                }
            }
            let ep = self.endpoint.clone();
            tokio::spawn(async move {
                let _ = ep.send_to(&addr, &forwarded).await;
            });
        }
        Ok(())
    }

    /// Dispatch du message interne de la cellule (post-decrypt).
    /// `cell.message` exclut le `circuit_id` (present en en-tete) : on
    /// le reinsere pour retrouver le payload `pack_serializable`
    /// complet attendu par les `unpack`.
    fn on_cell_message(self: &Arc<Self>, src: SocketAddr, cell: &Cell) -> Result<(), Ipv8Error> {
        let mut full = Vec::with_capacity(4 + cell.message.len());
        full.extend_from_slice(&cell.circuit_id.to_be_bytes());
        full.extend_from_slice(&cell.message);
        let mut r = Reader::new(&full);
        match cell.inner_msg_id {
            msg::CREATE => {
                let p = tp::Create::unpack(&mut r)?;
                self.on_create(src, p);
            }
            msg::CREATED => {
                let p = tp::Created::unpack(&mut r)?;
                self.on_created(cell.circuit_id, p);
            }
            msg::EXTEND => {
                let p = tp::Extend::unpack(&mut r)?;
                self.on_extend(src, p);
            }
            msg::EXTENDED => {
                let p = tp::Extended::unpack(&mut r)?;
                self.on_extended(cell.circuit_id, p)?;
            }
            msg::PING => {
                let p = tp::TunnelPing::unpack(&mut r)?;
                self.on_tunnel_ping(src, cell.circuit_id, p.identifier);
            }
            msg::PONG => {
                let _ = tp::TunnelPing::unpack(&mut r)?;
            }
            msg::DATA => {
                let p = tp::Data::unpack(&mut r)?;
                self.on_data(src, cell.circuit_id, p);
            }
            msg::ESTABLISH_INTRO => {
                let p = tp::EstablishIntro::unpack(&mut r)?;
                self.on_establish_intro(src, p, cell.circuit_id);
            }
            msg::INTRO_ESTABLISHED => {
                let p = tp::IntroEstablished::unpack(&mut r)?;
                self.on_intro_established(p);
            }
            msg::ESTABLISH_RENDEZVOUS => {
                let p = tp::EstablishRendezvous::unpack(&mut r)?;
                self.on_establish_rendezvous(src, p, cell.circuit_id);
            }
            msg::RENDEZVOUS_ESTABLISHED => {
                let p = tp::RendezvousEstablished::unpack(&mut r)?;
                self.on_rendezvous_established(p);
            }
            msg::LINK_E2E => {
                let p = tp::LinkE2E::unpack(&mut r)?;
                self.on_link_e2e(src, p, cell.circuit_id);
            }
            msg::LINKED_E2E => {
                let p = tp::LinkedE2E::unpack(&mut r)?;
                self.on_linked_e2e(p);
            }
            msg::PEERS_REQUEST => {
                let p = tp::PeersRequest::unpack(&mut r)?;
                self.on_peers_request(src, p, Some(cell.circuit_id));
            }
            msg::PEERS_RESPONSE => {
                let p = tp::PeersResponse::unpack(&mut r)?;
                self.on_peers_response(p);
            }
            msg::HTTP_REQUEST => {
                let p = tp::HttpRequest::unpack(&mut r)?;
                self.on_http_request(cell.circuit_id, p);
            }
            msg::HTTP_RESPONSE => {
                let p = tp::HttpResponse::unpack(&mut r)?;
                self.on_http_response(p);
            }
            // Cellules de speed-test : 19/20 = format pyipv8 pur
            // (`identifier` u16), 21/22 = format `ipv8-rust-tunnels`
            // (`identifier` u32 — le filaire reel de Tribler 8.x).
            msg::TEST_REQUEST => {
                let p = tp::TestRequest::unpack(&mut r)?;
                self.on_py_test_request(src, p);
            }
            msg::TEST_RESPONSE => {
                let p = tp::TestResponse::unpack(&mut r)?;
                self.on_py_test_response(p);
            }
            msg::SPEED_TEST_REQUEST => {
                let p = tp::SpeedTestRequest::unpack(&mut r)?;
                self.on_speedtest_request(src, p);
            }
            msg::SPEED_TEST_RESPONSE => {
                let p = tp::SpeedTestResponse::unpack(&mut r)?;
                // `cell.len()` Python : taille de la cellule sur le
                // fil = 30 octets d'en-tete + message interne.
                self.on_speedtest_response(p.identifier, 30 + cell.message.len());
            }
            _ => {
                tracing::trace!(msg_id = cell.inner_msg_id, "cellule ignoree");
            }
        }
        Ok(())
    }

    /// `extract_peer_flags` : `ExtraIntroductionPayload.flags` est le
    /// OR des `PEER_FLAG_*` serialise `>H` (packer `Flags` pyipv8).
    fn extract_peer_flags(extra_bytes: &[u8]) -> i32 {
        if extra_bytes.len() < 2 {
            return 0;
        }
        i32::from(u16::from_be_bytes([extra_bytes[0], extra_bytes[1]]))
    }

    /// `candidates[peer] = flags` : enregistre le pair + ses flags
    /// appris par une introduction signee sur le prefixe tunnel.
    ///
    /// `circuit.exit_flags` (pyipv8) est figé au moment ou le dernier
    /// saut repond au `create`/`extend` (`ours_on_created_extended`) :
    /// si l'introduction directe de ce pair (donc ses flags) n'est
    /// apprise qu'apres coup — cas frequent, le pair de sortie est
    /// souvent connu via la liste de candidats du saut precedent
    /// avant tout `walk` IPv8 direct — le circuit restait `READY`
    /// avec `exit_flags=0` pour toujours, invisible du selecteur
    /// SOCKS5 HTTP (`aucun circuit HTTP pret`). On rattrape donc les
    /// circuits deja construits dont le dernier saut correspond.
    fn register_tunnel_peer(self: &Arc<Self>, public_key_bin: &[u8], src: SocketAddr, flags: i32) {
        let Some(peer) = Peer::new(public_key_bin.to_vec(), Some(UdpAddress::from(src))) else {
            return;
        };
        self.network.add_verified(peer.clone());
        self.network
            .discover_service(&peer.public_key_bin, self.community_id);
        let mut updated = false;
        {
            let mut inner = self.inner.lock().unwrap();
            inner
                .flag_registry
                .insert(peer.public_key_bin.clone(), flags);
            for circuit in inner.circuits.values_mut() {
                if circuit
                    .hops
                    .last()
                    .is_some_and(|h| h.public_key_bin == peer.public_key_bin)
                    && circuit.exit_flags != flags
                {
                    circuit.exit_flags = flags;
                    updated = true;
                }
            }
        }
        if updated {
            self.notify_circuits_changed();
        }
    }

    /// Enregistre un pair de sortie restaure du `exitnode_cache`
    /// (`load_exit_nodes` Python : pairs exit connus reinjectes dans
    /// `Network` au demarrage, puis re-sollicites par introduction).
    pub fn register_exit_peer(&self, public_key_bin: &[u8], addr: SocketAddr, flags: i32) {
        // Meme chemin que `register_tunnel_peer` mais sans le
        // rattrapage `exit_flags` des circuits existants : au
        // demarrage (seul appelant du cache) il n'y a encore aucun
        // circuit.
        let Some(peer) = Peer::new(public_key_bin.to_vec(), Some(UdpAddress::from(addr))) else {
            return;
        };
        self.network.add_verified(peer.clone());
        self.network
            .discover_service(&peer.public_key_bin, self.community_id);
        self.inner
            .lock()
            .unwrap()
            .flag_registry
            .insert(peer.public_key_bin, flags);
    }

    /// `get_candidates(*flags)` : pairs connus portant `flag`
    /// (`PEER_FLAG_*`) dans leur bitmask annonce.
    pub fn get_candidates(&self, flag: i32) -> Vec<Peer> {
        let inner = self.inner.lock().unwrap();
        let candidates: Vec<Peer> = self
            .network
            .peers_for_service(&self.community_id)
            .into_iter()
            .filter(|p| {
                inner
                    .flag_registry
                    .get(&p.public_key_bin)
                    .is_some_and(|f| f & flag != 0)
            })
            .collect();
        self.filter_backup_exits(&inner, candidates, &[flag])
    }

    /// Flags annonces par un pair (0 si inconnu).
    pub fn peer_flags_of(&self, public_key_bin: &[u8]) -> i32 {
        self.inner
            .lock()
            .unwrap()
            .flag_registry
            .get(public_key_bin)
            .copied()
            .unwrap_or(0)
    }

    /// `community_id` (prefixe reseau de la community).
    pub fn community_id(&self) -> tribler_ipv8::CommunityId {
        self.community_id
    }

    /// `overlay.walk_to` (`Community.walk_to` → introduction-request
    /// sur le prefixe tunnel) — `/api/ipv8/isolation` "exitnode".
    pub async fn walk_to(&self, addr: &UdpAddress) -> Result<(), Ipv8Error> {
        self.send_introduction_request(addr).await
    }

    /// Injecte la `DiscoveryCommunity` de la stack (estimations
    /// WAN/LAN partagees, `my_peer` Python).
    pub fn set_discovery(&self, discovery: Arc<tribler_ipv8::discovery::DiscoveryCommunity>) {
        *self.discovery.lock().unwrap() = Some(discovery);
    }

    /// `my_estimated_wan` (via la discovery ; sinon non-specifie).
    fn my_wan(&self) -> UdpAddress {
        self.discovery
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.my_estimated_wan())
            .unwrap_or_else(UdpAddress::unspecified)
    }

    /// `my_estimated_lan` (via la discovery ; sinon non-specifie).
    fn my_lan(&self) -> UdpAddress {
        self.discovery
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.my_estimated_lan())
            .unwrap_or_else(UdpAddress::unspecified)
    }

    /// `RandomWalk.take_step` sous le prefixe tunnel (cible 20 pairs,
    /// `BaseLauncher.get_walk_strategies` Tribler) : introduction-
    /// request vers un pair connu de l'overlay (qui nous introduit a
    /// un pair de son choix), une adresse walkable du service, un
    /// pair verifie quelconque (un noeud Tribler porte toutes ses
    /// overlays sur le meme port) ou un noeud de bootstrap.
    ///
    /// Sans cette marche l'overlay tunnel ne se peuple que de pairs
    /// vus en traffic de circuits — les `ExtraIntroductionPayload`
    /// (flags RELAY/EXIT) arrivent trop tard pour elargir le pool de
    /// candidats au premier saut.
    pub async fn step(&self, bootstrap: &[UdpAddress]) -> Result<(), Ipv8Error> {
        const WALK_TARGET_PEERS: usize = 20;
        let known = self.network.peers_for_service(&self.community_id);
        if known.len() >= WALK_TARGET_PEERS {
            return Ok(());
        }
        let walkable = self
            .network
            .get_walkable_addresses(Some(&self.community_id), false);
        let target = {
            let mut rng = rand::thread_rng();
            if !known.is_empty() && (walkable.is_empty() || rand::random::<f64>() < 0.5) {
                known.choose(&mut rng).and_then(|p| p.address.clone())
            } else {
                walkable.choose(&mut rng).cloned().or_else(|| {
                    self.network
                        .verified_peers()
                        .choose(&mut rng)
                        .and_then(|p| p.address.clone())
                        .or_else(|| bootstrap.choose(&mut rng).cloned())
                })
            }
        };
        if let Some(addr) = target {
            self.walk_to(&addr).await?;
        }
        Ok(())
    }

    /// Boucle de marche aleatoire (a spawner) — `walker_interval`
    /// Python (`ipv8.walker_interval`).
    pub async fn run(self: &Arc<Self>, bootstrap: Vec<UdpAddress>, interval: Duration) {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = self.step(&bootstrap).await {
                tracing::debug!(error = %e, "etape de marche tunnel echouee");
            }
        }
    }

    /// `register_task("do_circuits", interval=5)` +
    /// `register_task("do_ping", interval=PING_INTERVAL)` +
    /// `register_task("do_peer_discovery", interval=10)` de
    /// `hidden_services.py` : boucle de maintenance a spawner.
    ///
    /// - `do_circuits` Python construit les circuits manquants — chez
    ///   nous cette demande vient des watchdogs de lanes (data) ; la
    ///   partie reprise ici est `do_remove` (nettoyage) + la garantie
    ///   d'un circuit `DATA` par nombre de sauts de swarm.
    /// - `do_ping` maintient les circuits (keepalive `ping`/`pong`).
    /// - `do_peer_discovery` : lookups PEX des swarms non-seedes et
    ///   `create_e2e` vers les points d'introduction connus.
    pub async fn run_maintenance(self: &Arc<Self>) {
        let mut circuits_tick = tokio::time::interval(Duration::from_secs(5));
        let mut ping_tick = tokio::time::interval(self.settings.ping_interval);
        let mut discovery_tick = tokio::time::interval(Duration::from_secs(10));
        for t in [&mut circuits_tick, &mut ping_tick, &mut discovery_tick] {
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        }
        // Premier tick immediat absorbe (rien a faire au demarrage).
        circuits_tick.tick().await;
        ping_tick.tick().await;
        discovery_tick.tick().await;
        loop {
            tokio::select! {
                _ = circuits_tick.tick() => {
                    self.do_remove().await;
                    self.ensure_swarm_data_circuits().await;
                }
                _ = ping_tick.tick() => self.do_ping().await,
                _ = discovery_tick.tick() => self.do_peer_discovery().await,
            }
        }
    }

    /// `do_remove` Python : retire les circuits/relais/sorties
    /// inactifs (`"no activity"`), trop vieux (`"too old"` via
    /// `get_max_time`) ou au-dela de `max_traffic` (avec `destroy`),
    /// et purge le registre de flags des pairs disparus de
    /// l'annuaire.
    async fn do_remove(self: &Arc<Self>) {
        enum Target {
            Circuit(u32),
            Relay(u32),
            Exit(u32),
        }
        impl Target {
            fn cid(&self) -> u32 {
                match self {
                    Target::Circuit(c) | Target::Relay(c) | Target::Exit(c) => *c,
                }
            }
        }
        let max_inactive = self.settings.max_time_inactive;
        let max_traffic = self.settings.max_traffic;
        let mut plan: Vec<(Target, &'static str, bool)> = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            // `get_max_time` inline (le verrou est deja pris) :
            // `max_time_ip` pour `IP_SEEDER` et les sorties servant
            // de point d'introduction, `max_time` sinon.
            let max_time_for = |cid: u32, ctype: Option<&str>| {
                if ctype == Some(crate::routing::CIRCUIT_TYPE_IP_SEEDER)
                    || inner
                        .intro_point_for
                        .values()
                        .any(|(icid, _)| *icid == cid)
                {
                    self.settings.max_time_ip
                } else {
                    self.settings.max_time
                }
            };
            for (cid, c) in inner.circuits.iter() {
                if c.state() == CIRCUIT_STATE_READY && c.base.is_inactive(max_inactive) {
                    plan.push((Target::Circuit(*cid), "no activity", false));
                } else if c.base.creation_time.elapsed() > max_time_for(*cid, Some(&c.ctype)) {
                    plan.push((Target::Circuit(*cid), "too old", false));
                } else if c.base.bytes_up + c.base.bytes_down > max_traffic {
                    plan.push((Target::Circuit(*cid), "traffic limit exceeded", true));
                }
            }
            for (cid, r) in inner.relays.iter() {
                if r.base.is_inactive(max_inactive) {
                    plan.push((Target::Relay(*cid), "no activity", false));
                } else if r.base.bytes_up + r.base.bytes_down > max_traffic {
                    plan.push((Target::Relay(*cid), "traffic limit exceeded", true));
                }
            }
            for (cid, e) in inner.exit_sockets.iter() {
                if e.last_activity.elapsed() > max_inactive {
                    plan.push((Target::Exit(*cid), "no activity", false));
                } else if e.creation_time.elapsed() > max_time_for(*cid, None) {
                    plan.push((Target::Exit(*cid), "too old", false));
                } else if e.bytes_total > max_traffic {
                    plan.push((Target::Exit(*cid), "traffic limit exceeded", true));
                }
            }
            // `do_remove` Python retire aussi les `candidates` absents
            // de `get_peers()` : equivalent — purge du `flag_registry`
            // des pairs disparus de l'annuaire.
            let known: std::collections::HashSet<Vec<u8>> = self
                .network
                .peers_for_service(&self.community_id)
                .iter()
                .map(|p| p.public_key_bin.clone())
                .collect();
            inner.flag_registry.retain(|pk, _| known.contains(pk));
        }

        for (target, info, destroy) in plan {
            if destroy {
                // `destroy=True` Python : prevenir le saut amont.
                let hop_addr = {
                    let inner = self.inner.lock().unwrap();
                    match &target {
                        Target::Circuit(cid) => inner
                            .circuits
                            .get(cid)
                            .and_then(|c| c.first_hop().and_then(|h| h.address.clone())),
                        Target::Relay(cid) => inner
                            .relays
                            .get(cid)
                            .and_then(|r| r.hop.address.clone()),
                        Target::Exit(cid) => inner
                            .exit_sockets
                            .get(cid)
                            .and_then(|e| e.hop.address.clone()),
                    }
                };
                if let Some(addr) = hop_addr {
                    let _ = self
                        .send_destroy(&addr, target.cid(), crate::routing::DESTROY_REASON_UNKNOWN)
                        .await;
                }
            }
            match target {
                Target::Circuit(cid) => self.remove_circuit(cid, info).await,
                Target::Relay(cid) => self.remove_relay(cid, info),
                Target::Exit(cid) => self.remove_exit_socket(cid, info),
            }
        }
    }

    /// `remove_relay` Python (retrait + `circuit_removed`) — le
    /// pendant entrant est `on_destroy`.
    fn remove_relay(&self, circuit_id: u32, additional_info: &str) {
        let ev = {
            let mut inner = self.inner.lock().unwrap();
            inner.relays.remove(&circuit_id).map(|route| {
                // La route correspondante disparait avec
                // (`relay_from_to` Python : les deux directions
                // partagent le sort).
                inner.relays.remove(&route.base.circuit_id);
                CircuitRemovedEvent {
                    circuit_id: route.base.circuit_id,
                    circuit_class: "RelayRoute",
                    bytes_up: route.base.bytes_up,
                    bytes_down: route.base.bytes_down,
                    uptime_secs: route.base.creation_time.elapsed().as_secs_f64(),
                    additional_info: additional_info.to_string(),
                }
            })
        };
        if let Some(ev) = ev {
            self.emit_circuit_removed(ev);
        }
    }

    /// `remove_exit_socket` Python (retrait + `circuit_removed` +
    /// purge intro/rendezvous via `cleanup_exit_socket`).
    fn remove_exit_socket(&self, circuit_id: u32, additional_info: &str) {
        let ev = {
            let mut inner = self.inner.lock().unwrap();
            inner.exit_sockets.remove(&circuit_id).map(|exit| {
                cleanup_exit_socket(&mut inner, circuit_id);
                CircuitRemovedEvent {
                    circuit_id,
                    circuit_class: "TunnelExitSocket",
                    bytes_up: 0,
                    bytes_down: exit.bytes_total,
                    uptime_secs: exit.creation_time.elapsed().as_secs_f64(),
                    additional_info: additional_info.to_string(),
                }
            })
        };
        if let Some(ev) = ev {
            self.emit_circuit_removed(ev);
        }
    }

    /// `do_ping` (`hidden_services.py`) : ping tous les circuits prets
    /// sauf `RP_SEEDER` et les `RP_DOWNLOADER` pas encore lies e2e.
    async fn do_ping(self: &Arc<Self>) {
        let targets: Vec<u32> = {
            let inner = self.inner.lock().unwrap();
            inner
                .circuits
                .values()
                .filter(|c| {
                    c.state() == CIRCUIT_STATE_READY
                        && c.ctype != crate::routing::CIRCUIT_TYPE_RP_SEEDER
                        && !(c.ctype == crate::routing::CIRCUIT_TYPE_RP_DOWNLOADER && !c.e2e)
                })
                .map(|c| c.base.circuit_id)
                .collect()
        };
        for cid in targets {
            let _ = self.send_ping(cid).await;
        }
    }

    /// `do_circuits` de `HiddenTunnelCommunity` : au moins un circuit
    /// `DATA` par nombre de sauts de swarm (communication avec les
    /// points d'introduction).
    async fn ensure_swarm_data_circuits(self: &Arc<Self>) {
        let hop_counts: Vec<usize> = {
            let inner = self.inner.lock().unwrap();
            inner
                .swarms
                .values()
                .map(|s| s.hops)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect()
        };
        for hops in hop_counts {
            let _ = self.build_circuits_if_needed(hops, 1).await;
        }
    }

    /// `on_puncture_request` generique (non signe, msg 250/232) :
    /// repond par un `puncture` signe vers `wan_walker` (ou
    /// `lan_walker` si meme IP WAN que nous) — c'est ce trou NAT qui
    /// permet a un pair introduit sous l'overlay tunnel de nous
    /// joindre.
    fn on_puncture_request(self: &Arc<Self>, pkt: &Packet) {
        let mut r = Reader::new(&pkt.payload);
        let (lan_walker, wan_walker, identifier, new_style) = match pkt.msg_id {
            ip::msg::PUNCTURE_REQUEST => match ip::PunctureRequestPayload::unpack(&mut r) {
                Ok(p) => (
                    p.lan_walker_address,
                    p.wan_walker_address,
                    p.identifier,
                    false,
                ),
                Err(_) => return,
            },
            ip::msg::NEW_PUNCTURE_REQUEST => match ip::NewPunctureRequestPayload::unpack(&mut r) {
                Ok(p) => (
                    p.lan_walker_address,
                    p.wan_walker_address,
                    p.identifier,
                    true,
                ),
                Err(_) => return,
            },
            _ => return,
        };
        let my_wan = self.my_wan();
        let same_wan = match (&wan_walker, &my_wan) {
            (UdpAddress::Ipv4(x), UdpAddress::Ipv4(y)) => x.ip() == y.ip(),
            (UdpAddress::Ipv6(x), UdpAddress::Ipv6(y)) => x.ip() == y.ip(),
            _ => false,
        };
        let target = if same_wan {
            lan_walker
        } else {
            wan_walker.clone()
        };
        let c = self.clone();
        let my_lan = self.my_lan();
        tokio::spawn(async move {
            let mut w = Writer::new();
            let msg_id = if new_style {
                let _ = w.ip_address(&my_lan);
                let _ = w.ip_address(&wan_walker);
                ip::msg::NEW_PUNCTURE
            } else {
                let _ = w.ipv4(&my_lan);
                let _ = w.ipv4(&wan_walker);
                ip::msg::PUNCTURE
            };
            w.u16(identifier);
            let pkt = Packet::sign(
                &c.community_id,
                msg_id,
                &c.key,
                c.claim_global_time(),
                &w.into_bytes(),
            );
            let _ = c.endpoint.send_to(&target, &pkt).await;
        });
    }

    /// Annuaire reseau partage.
    pub fn network(&self) -> &Arc<Network> {
        &self.network
    }

    /// `decode_map` `HiddenTunnelCommunity` pyipv8 : cellules (0),
    /// destroy (8) et messages e2e/signales (13/17/18) + map de base.
    fn tunnel_msg_name(msg_id: u8) -> Option<&'static str> {
        match msg_id {
            0 => Some("on_cell"),
            8 => Some("on_destroy"),
            13 => Some("on_create_e2e"),
            17 => Some("on_peers_request"),
            18 => Some("on_peers_response"),
            _ => tribler_ipv8::overlays::base_community_msg_name(msg_id),
        }
    }

    /// `OverlaySchema` : instantane REST de la community
    /// (`GET /api/ipv8/overlays`) — `TriblerTunnelCommunity`.
    pub fn overlay_info(&self, is_isolated: bool) -> tribler_ipv8::overlays::OverlayInfo {
        use tribler_ipv8::overlays::{
            overlay_peer, OverlayInfo, OverlayStrategy, DEFAULT_MAX_PEERS,
        };
        OverlayInfo {
            community_id: self.community_id,
            my_peer_hex: hex::encode(self.key.public_key().to_bin()),
            global_time: self.global_time.load(std::sync::atomic::Ordering::Relaxed),
            peers: self
                .network
                .peers_for_service(&self.community_id)
                .iter()
                .map(overlay_peer)
                .collect(),
            overlay_name: "TriblerTunnelCommunity",
            max_peers: DEFAULT_MAX_PEERS,
            is_isolated,
            // `my_estimated_*` non suivis par cette community
            // (`DiscoveryCommunity` estime ; repli 0.0.0.0:0).
            my_estimated_wan: UdpAddress::unspecified(),
            my_estimated_lan: UdpAddress::unspecified(),
            // `BaseLauncher.get_walk_strategies` Tribler.
            strategies: vec![OverlayStrategy {
                name: "RandomWalk",
                target_peers: 20,
            }],
            decode: Self::tunnel_msg_name,
        }
    }

    /// `create_introduction_request` : requete signee sur le prefixe
    /// tunnel avec `extra_bytes` = nos `peer_flags` (`>H` — packer
    /// `Flags`). A utiliser pour decouvrir les flags d'un pair
    /// connu (`network`) avant de l'integrer comme saut/sortie.
    pub async fn send_introduction_request(&self, addr: &UdpAddress) -> Result<(), Ipv8Error> {
        let local = UdpAddress::from(self.endpoint.local_addr()?);
        let (lan, wan) = (self.my_lan(), self.my_wan());
        let my_lan = if lan.is_unspecified() {
            local.clone()
        } else {
            lan
        };
        let my_wan = if wan.is_unspecified() { local } else { wan };
        let mut w = Writer::new();
        ip::IntroductionRequest {
            destination_address: addr.clone(),
            source_lan_address: my_lan,
            source_wan_address: my_wan,
            advice: true,
            supports_new_style: true,
            connection_type: ip::ConnectionType::Unknown,
            identifier: self.next_id(),
            extra_bytes: (self.inner.lock().unwrap().peer_flags as u16)
                .to_be_bytes()
                .to_vec(),
        }
        .pack(&mut w)?;
        let pkt = Packet::sign(
            &self.community_id,
            ip::IntroductionRequest::MSG_ID,
            &self.key,
            self.claim_global_time() % 65536,
            &w.into_bytes(),
        );
        self.endpoint.send_to(addr, &pkt).await
    }

    /// `introduction_request_callback` + reponse : enregistre les
    /// flags du demandeur, marque le pair service tunnel, renvoie une
    /// `introduction-response` (meme style que la requete) portant nos
    /// flags et un pair introduit de l'overlay
    /// (`get_peer_for_introduction`), plus une `puncture-request` non
    /// signee vers le pair introduit (`create_introduction_response`
    /// Python — le trou NAT est perce par le puncture).
    /// Parametres decodes d'une `introduction-request` (ancien ou
    /// nouveau format).
    fn on_introduction_request(
        self: &Arc<Self>,
        src: SocketAddr,
        public_key_bin: &[u8],
        req: IntroRequestParams<'_>,
    ) {
        let (identifier, source_lan, source_wan, new_style, extra_bytes) = (
            req.identifier,
            req.source_lan,
            req.source_wan,
            req.new_style,
            req.extra_bytes,
        );
        let flags = Self::extract_peer_flags(extra_bytes);
        self.register_tunnel_peer(public_key_bin, src, flags);

        let candidates: Vec<Peer> = self
            .network
            .peers_for_service(&self.community_id)
            .into_iter()
            .filter(|q| q.public_key_bin != public_key_bin)
            .filter(|q| q.address.as_ref().is_some_and(|a| !a.is_unspecified()))
            .collect();
        let introduction = candidates.choose(&mut rand::thread_rng()).cloned();
        let (lan_i, wan_i) = match introduction.as_ref().and_then(|p| p.address.clone()) {
            Some(a) => (a.clone(), a),
            None => (UdpAddress::unspecified(), UdpAddress::unspecified()),
        };

        let c = self.clone();
        let dst = UdpAddress::from(src);
        let dest_addr = source_wan.clone();
        let my_lan = {
            let l = self.my_lan();
            if l.is_unspecified() {
                self.endpoint
                    .local_addr()
                    .map(UdpAddress::from)
                    .unwrap_or_else(|_| UdpAddress::unspecified())
            } else {
                l
            }
        };
        let my_wan = {
            let w = self.my_wan();
            if w.is_unspecified() {
                self.endpoint
                    .local_addr()
                    .map(UdpAddress::from)
                    .unwrap_or_else(|_| UdpAddress::unspecified())
            } else {
                w
            }
        };
        tokio::spawn(async move {
            let flags_bytes = (c.inner.lock().unwrap().peer_flags as u16)
                .to_be_bytes()
                .to_vec();
            let mut w = Writer::new();
            let (res, msg_id) = if new_style {
                (
                    ip::NewIntroductionResponse {
                        destination_address: dest_addr,
                        source_lan_address: my_lan,
                        source_wan_address: my_wan,
                        lan_introduction_address: lan_i,
                        wan_introduction_address: wan_i,
                        identifier,
                        intro_supports_new_style: false,
                        extra_bytes: flags_bytes,
                    }
                    .pack(&mut w),
                    ip::NewIntroductionResponse::MSG_ID,
                )
            } else {
                (
                    ip::IntroductionResponse {
                        destination_address: dest_addr,
                        source_lan_address: my_lan,
                        source_wan_address: my_wan,
                        lan_introduction_address: lan_i,
                        wan_introduction_address: wan_i,
                        connection_type: ip::ConnectionType::Unknown,
                        supports_new_style: true,
                        peer_limit_reached: false,
                        identifier,
                        intro_supports_new_style: false,
                        extra_bytes: flags_bytes,
                    }
                    .pack(&mut w),
                    ip::IntroductionResponse::MSG_ID,
                )
            };
            if res.is_ok() {
                let pkt = Packet::sign(
                    &c.community_id,
                    msg_id,
                    &c.key,
                    c.claim_global_time() % 65536,
                    &w.into_bytes(),
                );
                let _ = c.endpoint.send_to(&dst, &pkt).await;
            }
        });

        // `puncture-request` vers le pair introduit pour qu'il perce
        // son NAT vers le demandeur.
        if let Some(intro) = introduction {
            if let Some(intro_addr) = intro.address {
                let c = self.clone();
                let req_lan = source_lan.clone();
                let req_wan = source_wan.clone();
                tokio::spawn(async move {
                    let mut w = Writer::new();
                    let msg_id = if new_style
                        || !matches!(req_lan, UdpAddress::Ipv4(_))
                        || !matches!(req_wan, UdpAddress::Ipv4(_))
                    {
                        let _ = w.ip_address(&req_lan);
                        let _ = w.ip_address(&req_wan);
                        ip::msg::NEW_PUNCTURE_REQUEST
                    } else {
                        let _ = w.ipv4(&req_lan);
                        let _ = w.ipv4(&req_wan);
                        ip::msg::PUNCTURE_REQUEST
                    };
                    w.u16(identifier);
                    let pkt = Packet::pack_unsigned(
                        &c.community_id,
                        msg_id,
                        c.claim_global_time(),
                        &w.into_bytes(),
                    );
                    let _ = c.endpoint.send_to(&intro_addr, &pkt).await;
                });
            }
        }
    }

    /// `introduction_response_callback` : enregistre les flags de
    /// l'emetteur de la reponse et inscrit les adresses introduites
    /// en walkables du service tunnel (`introductions` Python).
    fn on_introduction_response(
        self: &Arc<Self>,
        src: SocketAddr,
        public_key_bin: &[u8],
        lan_introduction: UdpAddress,
        wan_introduction: UdpAddress,
        new_style: bool,
        extra_bytes: &[u8],
    ) {
        let flags = Self::extract_peer_flags(extra_bytes);
        self.register_tunnel_peer(public_key_bin, src, flags);
        if let Some(peer) = Peer::new(public_key_bin.to_vec(), Some(UdpAddress::from(src))) {
            for addr in [lan_introduction, wan_introduction] {
                if !addr.is_unspecified() {
                    self.network
                        .discover_address(&peer, addr, Some(self.community_id), new_style);
                }
            }
        }
    }

    /// Paquet IPv8 (signe ou non) recu sur le prefixe tunnel :
    /// `destroy`, introductions (suivi des flags de sortie) et
    /// messages E2E (non-cellules).
    fn on_packet(self: &Arc<Self>, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        if !pkt.signed {
            // `puncture-request` (250/232) est le seul message non
            // signe de la marche — y repondre permet a un pair
            // introduit de percer son NAT vers nous.
            self.on_puncture_request(&pkt);
            return Ok(());
        }
        match pkt.msg_id {
            msg::DESTROY => {
                let mut r = Reader::new(&pkt.payload);
                let d = tp::Destroy::unpack(&mut r)?;
                self.on_destroy(src, d.circuit_id, d.reason);
            }
            x if x == ip::IntroductionRequest::MSG_ID => {
                let mut r = Reader::new(&pkt.payload);
                let p = ip::IntroductionRequest::unpack(&mut r)?;
                self.on_introduction_request(
                    src,
                    &pkt.public_key_bin,
                    IntroRequestParams {
                        identifier: p.identifier,
                        source_lan: p.source_lan_address,
                        source_wan: p.source_wan_address,
                        new_style: false,
                        extra_bytes: &p.extra_bytes,
                    },
                );
            }
            x if x == ip::NewIntroductionRequest::MSG_ID => {
                let mut r = Reader::new(&pkt.payload);
                let p = ip::NewIntroductionRequest::unpack(&mut r)?;
                self.on_introduction_request(
                    src,
                    &pkt.public_key_bin,
                    IntroRequestParams {
                        identifier: p.identifier,
                        source_lan: p.source_lan_address,
                        source_wan: p.source_wan_address,
                        new_style: true,
                        extra_bytes: &p.extra_bytes,
                    },
                );
            }
            x if x == ip::IntroductionResponse::MSG_ID => {
                let mut r = Reader::new(&pkt.payload);
                let p = ip::IntroductionResponse::unpack(&mut r)?;
                self.on_introduction_response(
                    src,
                    &pkt.public_key_bin,
                    p.lan_introduction_address,
                    p.wan_introduction_address,
                    false,
                    &p.extra_bytes,
                );
            }
            x if x == ip::NewIntroductionResponse::MSG_ID => {
                let mut r = Reader::new(&pkt.payload);
                let p = ip::NewIntroductionResponse::unpack(&mut r)?;
                self.on_introduction_response(
                    src,
                    &pkt.public_key_bin,
                    p.lan_introduction_address,
                    p.wan_introduction_address,
                    true,
                    &p.extra_bytes,
                );
            }
            _ => {
                tracing::trace!(msg_id = pkt.msg_id, %src, "paquet tunnel ignore");
            }
        }
        Ok(())
    }

    /// `on_create` : un pair demande a rejoindre notre segment.
    fn on_create(self: &Arc<Self>, src: SocketAddr, p: tp::Create) {
        let joined = {
            let inner = self.inner.lock().unwrap();
            inner.relays.len() + inner.exit_sockets.len()
        };
        if self.inner.lock().unwrap().peer_flags == 0
            || joined >= self.settings.max_joined_circuits
            || self
                .inner
                .lock()
                .unwrap()
                .created_requests
                .contains_key(&p.circuit_id)
        {
            tracing::debug!("create ignore circuit {}", p.circuit_id);
            return;
        }
        let c = self.clone();
        tokio::spawn(async move {
            if let Err(e) = c.join_circuit(src, p).await {
                tracing::debug!(error = %e, "join_circuit echoue");
            }
        });
    }

    /// `join_circuit` : DH partage + `created` + exit socket local.
    async fn join_circuit(
        self: &Arc<Self>,
        src: SocketAddr,
        p: tp::Create,
    ) -> Result<(), Ipv8Error> {
        let circuit_id = p.circuit_id;
        let ds = generate_diffie_shared_secret(&p.key, &self.key)?;
        let session_keys = generate_session_keys(&ds.shared)?;

        let requester = Peer::new(p.node_public_key.clone(), Some(UdpAddress::from(src)))
            .ok_or(Ipv8Error::Malformed("cle de requete invalide"))?;

        // Candidats proposes : relays puis sorties (marqueur = premiere
        // sortie dupliquee — convention `join_circuit` Python). Les
        // sorties sont celles dont les `PEER_FLAG_EXIT_*` sont connus
        // via le suivi des flags (`flag_registry`).
        let peers = self.network.peers_for_service(&self.community_id);
        let flags_snapshot: HashMap<Vec<u8>, i32> =
            self.inner.lock().unwrap().flag_registry.clone();
        let (exits, relays_only): (Vec<Peer>, Vec<Peer>) = peers
            .into_iter()
            .filter(|q| q.public_key_bin != requester.public_key_bin)
            .partition(|q| {
                flags_snapshot
                    .get(&q.public_key_bin)
                    .is_some_and(|f| f & ANY_EXIT_FLAGS != 0)
            });
        let mut list: Vec<Peer> = relays_only
            .into_iter()
            .take(CANDIDATES_IN_RESPONSE)
            .collect();
        let exit_list: Vec<Peer> = exits.into_iter().take(CANDIDATES_IN_RESPONSE).collect();
        if let Some(first) = exit_list.first().cloned() {
            let mut marked = vec![first];
            marked.extend(exit_list);
            list.extend(marked);
        }
        let candidates: HashMap<Vec<u8>, Peer> = list
            .iter()
            .map(|q| (q.public_key_bin.clone(), q.clone()))
            .collect();
        let mut keys_w = Writer::new();
        keys_w.u8(list.len() as u8);
        for q in &list {
            keys_w.varlen_h(&q.public_key_bin);
        }
        let mut enc_keys = session_keys.clone();
        let candidates_enc = enc_keys.encrypt_str(&keys_w.into_bytes(), Direction::Forward)?;

        // Socket de sortie dediee (`TunnelExitSocket` : socket UDP
        // propre recevant les reponses hors-prefixe des destinations).
        let exit_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        let exit_socket = Arc::new(exit_socket);
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.created_requests.insert(
                circuit_id,
                CreatedRequest {
                    peer: requester.clone(),
                    candidates,
                },
            );
            inner.exit_sockets.insert(
                circuit_id,
                ExitState {
                    hop: Hop {
                        public_key_bin: requester.public_key_bin.clone(),
                        address: Some(UdpAddress::from(src)),
                        session_keys,
                    },
                    enabled: false,
                    socket: exit_socket.clone(),
                    _stop_tx: stop_tx,
                    http_permits: Arc::new(tokio::sync::Semaphore::new(
                        crate::http_tunnel::MAX_HTTP_REQUESTS_PER_CIRCUIT,
                    )),
                    creation_time: std::time::Instant::now(),
                    last_activity: std::time::Instant::now(),
                    bytes_total: 0,
                },
            );
        }
        self.spawn_exit_recv(circuit_id, exit_socket, stop_rx);

        let reply = tp::Created {
            circuit_id,
            identifier: p.identifier,
            key: ds.crypt_pk.to_vec(),
            auth: ds.auth,
            candidates_enc,
        };
        self.send_cell(&UdpAddress::from(src), &reply)
            .await
            .map(|_| ())
    }

    /// `on_created` : soit a relayer (extend en cours), soit reponse a
    /// notre propre create.
    fn on_created(self: &Arc<Self>, circuit_id: u32, p: tp::Created) {
        let relay_ctx = {
            self.inner
                .lock()
                .unwrap()
                .create_requests
                .remove(&p.identifier)
        };
        if let Some(req) = relay_ctx {
            self.relay_created(req, circuit_id, p);
            return;
        }
        let pending = {
            self.inner
                .lock()
                .unwrap()
                .retry_requests
                .get(&circuit_id)
                .map(|e| e.identifier)
        };
        if pending != Some(p.identifier) {
            tracing::debug!("created inattendu circuit {}", circuit_id);
            return;
        }
        self.ours_on_created_extended(circuit_id, &p.key, &p.auth, &p.candidates_enc);
    }

    /// Branche "relais" de `on_created` : transforme le `created` en
    /// `extended` vers l'amont et installe les deux routes de relais.
    fn relay_created(self: &Arc<Self>, req: CreateRequest, _to_cid: u32, p: tp::Created) {
        let exit = {
            self.inner
                .lock()
                .unwrap()
                .exit_sockets
                .remove(&req.from_circuit_id)
        };
        let Some(exit) = exit else {
            tracing::debug!("created pour exit socket inconnu {}", req.from_circuit_id);
            return;
        };
        let session_keys = exit.hop.session_keys;
        let upstream_addr = match req.peer.address.clone() {
            Some(a) => a,
            None => return,
        };
        {
            let mut inner = self.inner.lock().unwrap();
            // Cellules venant de l'aval (to_circuit_id) -> renvoyees a
            // l'amont (req.peer) sur from_circuit_id, direction BACKWARD.
            inner.relays.insert(
                req.to_circuit_id,
                RelayRoute {
                    base: RoutingObject::new(req.from_circuit_id),
                    hop: Hop {
                        public_key_bin: req.peer.public_key_bin.clone(),
                        address: req.peer.address.clone(),
                        session_keys: session_keys.clone(),
                    },
                    direction: Direction::Backward,
                    rendezvous_relay: false,
                    relay_early_count: 0,
                },
            );
            // Cellules venant de l'amont (from_circuit_id) -> aval
            // (req.to_peer) sur to_circuit_id, direction FORWARD.
            inner.relays.insert(
                req.from_circuit_id,
                RelayRoute {
                    base: RoutingObject::new(req.to_circuit_id),
                    hop: Hop {
                        public_key_bin: req.to_peer.public_key_bin.clone(),
                        address: req.to_peer.address.clone(),
                        session_keys,
                    },
                    direction: Direction::Forward,
                    rendezvous_relay: false,
                    relay_early_count: 0,
                },
            );
        }
        // `extended` envoye a l'amont : circuit_id = from_circuit_id,
        // chiffre BACKWARD (route de retour via `send_cell`).
        let ext = tp::Extended {
            circuit_id: req.from_circuit_id,
            identifier: req.extend_identifier,
            key: p.key,
            auth: p.auth,
            candidates_enc: p.candidates_enc,
        };
        let c = self.clone();
        tokio::spawn(async move {
            let _ = c.send_cell(&upstream_addr, &ext).await;
        });
    }

    /// `on_extended` : reponse a notre `extend` (initiateur).
    fn on_extended(self: &Arc<Self>, circuit_id: u32, p: tp::Extended) -> Result<(), Ipv8Error> {
        let pending = {
            self.inner
                .lock()
                .unwrap()
                .retry_requests
                .get(&circuit_id)
                .map(|e| e.identifier)
        };
        if pending != Some(p.identifier) {
            return Err(Ipv8Error::Malformed("extended inattendu"));
        }
        self.ours_on_created_extended(circuit_id, &p.key, &p.auth, &p.candidates_enc);
        Ok(())
    }

    /// `_ours_on_created_extended` : le saut a repondu — verifie
    /// l'auth DH, derive les cles, ajoute le hop, poursuit l'extension
    /// ou marque le circuit READY.
    fn ours_on_created_extended(
        self: &Arc<Self>,
        circuit_id: u32,
        key: &[u8],
        auth: &[u8; 32],
        candidates_enc: &[u8],
    ) {
        let (dh_secret, hop_pk_bin, hop_addr) = {
            let inner = self.inner.lock().unwrap();
            let Some(c) = inner.circuits.get(&circuit_id) else {
                return;
            };
            let Some(uh) = &c.unverified_hop else {
                return;
            };
            (uh.dh_secret, uh.public_key_bin.clone(), uh.address.clone())
        };

        // `b` = crypt_pk X25519 de la cle publique declaree du hop.
        let Ok(hop_pk) = tribler_crypto::ipv8::keys::LibNaClPublicKey::from_bin(&hop_pk_bin) else {
            tracing::debug!("cle publique de hop invalide circuit {}", circuit_id);
            return;
        };
        let Ok(shared) = verify_and_generate_shared_secret(&dh_secret, key, auth, &hop_pk.crypt_pk)
        else {
            tracing::debug!("auth DH invalide circuit {}", circuit_id);
            return;
        };
        let Ok(session_keys) = generate_session_keys(&shared) else {
            return;
        };

        // Dechiffre la liste de candidats (FORWARD, avec les cles du
        // hop qui vient de repondre).
        let cand_bin = session_keys
            .decrypt_str(candidates_enc, Direction::Forward)
            .unwrap_or_default();
        let mut cr = Reader::new(&cand_bin);
        let cand_count = cr.u8().unwrap_or(0) as usize;
        let mut cand_keys: Vec<Vec<u8>> = Vec::with_capacity(cand_count);
        for _ in 0..cand_count {
            match cr.varlen_h() {
                Ok(k) => cand_keys.push(k.to_vec()),
                Err(_) => break,
            }
        }
        // Split relays/sorties : le premier element repete marque le
        // debut des candidats de sortie (convention Python).
        let mut relay_keys: &[Vec<u8>] = &cand_keys;
        let mut exit_keys: &[Vec<u8>] = &[];
        for i in 0..cand_keys.len().saturating_sub(1) {
            if cand_keys[i] == cand_keys[i + 1] {
                relay_keys = &cand_keys[..i];
                exit_keys = &cand_keys[i + 1..];
                break;
            }
        }

        let (goal, hops_done, become_exit) = {
            let mut inner = self.inner.lock().unwrap();
            // `circuit.exit_flags` pyipv8 : flags annonces du dernier
            // saut verifie (0 si le pair n'a rien publie) — lu avant
            // l'emprunt mutable de `circuits`.
            let flags = inner.flag_registry.get(&hop_pk_bin).copied().unwrap_or(0);
            let Some(circuit) = inner.circuits.get_mut(&circuit_id) else {
                return;
            };
            circuit.add_hop(Hop {
                public_key_bin: hop_pk_bin,
                address: hop_addr,
                session_keys,
            });
            circuit.exit_flags = flags;
            let goal = circuit.goal_hops;
            let done = circuit.hops.len();
            (goal, done, goal.saturating_sub(1) == done)
        };
        self.notify_circuits_changed();

        if hops_done < goal {
            // Choix du prochain candidat (`_ours_on_created_extended`
            // Python) : sorties si le prochain hop est le dernier,
            // sinon relays (fallback sorties). La liste complete est
            // passee a `send_extend` pour le retry sur alternates.
            let wanted: &[Vec<u8>] = if become_exit {
                exit_keys
            } else if !relay_keys.is_empty() {
                relay_keys
            } else {
                exit_keys
            };
            // `cache.max_tries if cache else 1` : les essais restants
            // du cache retry courant alimentent l'extend.
            let max_tries = {
                let inner = self.inner.lock().unwrap();
                inner
                    .retry_requests
                    .get(&circuit_id)
                    .map(|e| e.max_tries)
                    .unwrap_or(1)
            };
            let keys: Vec<Vec<u8>> = wanted.to_vec();
            let c = self.clone();
            tokio::spawn(async move {
                let _ = c.send_extend(circuit_id, keys, max_tries).await;
            });
        } else {
            self.inner
                .lock()
                .unwrap()
                .retry_requests
                .remove(&circuit_id);
        }
    }

    /// `on_extend` : on est le dernier hop — relaie un `create` vers
    /// le candidat demande.
    fn on_extend(self: &Arc<Self>, src: SocketAddr, p: tp::Extend) {
        let (peer_flags_ok, request) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.peer_flags & PEER_FLAG_RELAY != 0,
                inner
                    .created_requests
                    .get(&p.circuit_id)
                    .map(|r| (r.peer.clone(), r.candidates.clone())),
            )
        };
        if !peer_flags_ok {
            tracing::debug!("extend ignore : sans flag RELAY");
            return;
        }
        let Some((upstream_peer, candidates)) = request else {
            tracing::debug!("extend inattendu circuit {}", p.circuit_id);
            return;
        };

        let zero_addr = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
        let node_is_zero = p
            .node_addr
            .to_socket_addr()
            .map(|a| a == zero_addr)
            .unwrap_or(true);
        let extend_peer = if let Some(cand) = candidates.get(&p.node_public_key) {
            Some(cand.clone())
        } else if node_is_zero {
            None
        } else {
            self.network
                .get_by_key(&p.node_public_key)
                .or_else(|| Peer::new(p.node_public_key.clone(), Some(p.node_addr.clone())))
        };
        let Some(extend_peer) = extend_peer else {
            tracing::debug!("candidat d'extension inconnu");
            return;
        };
        let Some(down_addr) = extend_peer.address.clone() else {
            return;
        };
        let _ = src;

        let to_circuit_id = self.gen_circuit_id();
        let create_identifier = self.next_id();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.create_requests.insert(
                create_identifier,
                CreateRequest {
                    to_circuit_id,
                    from_circuit_id: p.circuit_id,
                    peer: upstream_peer,
                    to_peer: extend_peer.clone(),
                    extend_identifier: p.identifier,
                },
            );
        }
        let c = self.clone();
        let my_pk = self.key.public_key().to_bin();
        tokio::spawn(async move {
            let create = tp::Create {
                circuit_id: to_circuit_id,
                identifier: create_identifier,
                node_public_key: my_pk,
                key: p.key,
            };
            let _ = c.send_cell(&down_addr, &create).await;
        });
    }

    /// `on_data` : cellule data decryptee. Cote sortie -> envoi UDP
    /// brut ; cote initiateur -> livraison au consommateur.
    fn on_data(self: &Arc<Self>, src: SocketAddr, circuit_id: u32, p: tp::Data) {
        let (is_exit, is_e2e, hop_addr) = {
            let inner = self.inner.lock().unwrap();
            let is_exit = inner.exit_sockets.contains_key(&circuit_id);
            let c = inner.circuits.get(&circuit_id);
            (
                is_exit,
                c.map(|c| {
                    c.ctype == crate::routing::CIRCUIT_TYPE_RP_DOWNLOADER
                        || c.ctype == crate::routing::CIRCUIT_TYPE_RP_SEEDER
                })
                .unwrap_or(false),
                c.and_then(|c| c.first_hop().and_then(|h| h.address.clone())),
            )
        };
        if is_exit {
            self.exit_data(circuit_id, src, &p);
            return;
        }
        // Circuit nous appartenant : paquet tunnel prefixe embarque
        // (`could_be_ipv8`) -> `on_packet_from_circuit` ; circuits
        // e2e (RP_*) livrent la donnee brute (`on_raw_data`).
        let prefix = prefix_of(&self.community_id);
        let from_hop = hop_addr.as_ref().and_then(|a| a.to_socket_addr()) == Some(src);
        if from_hop && !is_e2e && p.data.len() > prefix.len() && p.data[..prefix.len()] == prefix {
            // `source_address` Python = `org_address` du payload (le
            // requester logique), PAS l'emetteur immediat de la
            // cellule.
            let origin = p.org_address.to_socket_addr().unwrap_or(src);
            self.on_packet_from_circuit(origin, &p.data, Some(circuit_id));
            return;
        }
        // `data_to_socks5` (`routing/circuit.rs` des tunnels Rust) :
        // sur les circuits e2e l'origine presentee au client est
        // reecrite en `circuit_id_to_ip(circuit_id):CIRCUIT_ID_PORT`
        // pour que les frames de reponse soient reroutees sur le meme
        // circuit.
        let origin = if is_e2e {
            UdpAddress::from(SocketAddr::V4(std::net::SocketAddrV4::new(
                crate::routing::circuit_id_to_ip(circuit_id),
                crate::routing::CIRCUIT_ID_PORT,
            )))
        } else {
            p.org_address
        };
        let msg = CircuitData {
            circuit_id,
            source: src,
            destination: p.dest_address,
            origin,
            data: p.data,
        };
        let sub = {
            let inner = self.inner.lock().unwrap();
            inner.data_subscribers.get(&circuit_id).cloned()
        };
        match sub {
            Some(tx) => {
                let _ = tx.try_send(msg);
            }
            None => {
                let _ = self.data_tx.send(msg);
            }
        }
    }

    /// `exit_data` : activation au premier octet vu du bon IP puis
    /// envoi UDP brut vers la destination via la socket dediee.
    fn exit_data(&self, circuit_id: u32, src: SocketAddr, p: &tp::Data) {
        let socket = {
            let mut inner = self.inner.lock().unwrap();
            let Some(exit) = inner.exit_sockets.get_mut(&circuit_id) else {
                return;
            };
            if !exit.enabled {
                let hop_ip = exit
                    .hop
                    .address
                    .as_ref()
                    .and_then(|a| a.to_socket_addr())
                    .map(|a| a.ip());
                if hop_ip == Some(src.ip()) {
                    exit.enabled = true;
                }
            }
            if !exit.enabled {
                return;
            }
            exit.last_activity = std::time::Instant::now();
            exit.bytes_total += p.data.len() as u64;
            exit.socket.clone()
        };
        // `is_allowed` (pyipv8 `DataChecker` + flags de sortie) : la
        // sortie ne relaie que du trafic BT/IPv8 reconnaissable — un
        // circuit ne doit pas devenir un proxy UDP generique.
        let flags = self.inner.lock().unwrap().peer_flags;
        let prefix = prefix_of(&self.community_id);
        if !tribler_network_policy::exit_policy::is_exit_data_allowed(&p.data, flags, &prefix) {
            tracing::debug!(
                circuit_id,
                "exit_data rejete : donnee hors politique de sortie"
            );
            return;
        }
        tracing::trace!(circuit_id, dest = ?p.dest_address, "exit_data");
        let Some(dest_sa) = p.dest_address.to_socket_addr() else {
            return;
        };
        let data = p.data.clone();
        tokio::spawn(async move {
            let _ = socket.send_to(&data, dest_sa).await;
        });
    }

    /// Tache de reception de la socket de sortie : les datagrammes
    /// hors-prefixe revenant de l'exterieur sont reencapsules en
    /// cellules `data` et renvoyes dans le tunnel (chiffrement
    /// BACKWARD vers l'amont).
    fn spawn_exit_recv(
        self: &Arc<Self>,
        circuit_id: u32,
        socket: Arc<tokio::net::UdpSocket>,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let c = self.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; crate::cell::MAX_CELL_WIRE];
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    recv = socket.recv_from(&mut buf) => {
                        let Ok((n, src)) = recv else { break };
                        c.exit_recv_data(circuit_id, src, &buf[..n]).await;
                    }
                }
            }
        });
    }

    /// Reencapsulation cote sortie : datagramme externe -> cellule
    /// `data` vers l'amont. Comme `TunnelExitSocket.tunnel_data`
    /// Python : `dest = 0.0.0.0:0` ("pour l'initiateur du circuit"),
    /// `org = source UDP reelle` du datagramme (pas de back_map).
    async fn exit_recv_data(&self, circuit_id: u32, src: SocketAddr, data: &[u8]) {
        let upstream_addr = {
            let mut inner = self.inner.lock().unwrap();
            let Some(exit) = inner.exit_sockets.get_mut(&circuit_id) else {
                return;
            };
            if !exit.enabled {
                return;
            }
            exit.last_activity = std::time::Instant::now();
            exit.bytes_total += data.len() as u64;
            match exit.hop.address.clone() {
                Some(a) => a,
                None => return,
            }
        };
        // `is_allowed` est applique dans les deux sens par pyipv8
        // (`sendto` ET `datagram_received`) : une reponse externe non
        // conforme ne doit pas non plus retourner dans le tunnel.
        let flags = self.inner.lock().unwrap().peer_flags;
        let prefix = prefix_of(&self.community_id);
        if !tribler_network_policy::exit_policy::is_exit_data_allowed(data, flags, &prefix) {
            tracing::debug!(circuit_id, "exit_recv_data rejete : reponse hors politique");
            return;
        }
        let p = tp::Data {
            circuit_id,
            dest_address: UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap()),
            org_address: UdpAddress::from(src),
            data: data.to_vec(),
        };
        let _ = self.send_cell(&upstream_addr, &p).await;
    }

    /// `on_destroy` : nettoie circuit/relais/sortie concernes.
    /// `watch_circuit_removed` : abonnement au canal
    /// `circuit_removed` (relaye en `tunnel_removed` cote SSE par
    /// `tribler-core`, sans dependance tunnel -> core).
    pub fn watch_circuit_removed(&self) -> tokio::sync::broadcast::Receiver<CircuitRemovedEvent> {
        self.circuit_removed_tx.subscribe()
    }

    /// `remove_circuit`/`remove_relay`/`remove_exit_socket` pyipv8 :
    /// emet `circuit_removed` avec les stats de l'objet detruit.
    fn emit_circuit_removed(&self, ev: CircuitRemovedEvent) {
        // send() n'echoue que sans abonnes — non bloquant par design.
        let _ = self.circuit_removed_tx.send(ev);
    }

    fn on_destroy(&self, _src: SocketAddr, circuit_id: u32, reason: u16) {
        let mut events = Vec::new();
        let mut inner = self.inner.lock().unwrap();
        if let Some(route) = inner.relays.remove(&circuit_id) {
            inner.relays.remove(&route.base.circuit_id);
            events.push(CircuitRemovedEvent {
                circuit_id: route.base.circuit_id,
                circuit_class: "RelayRoute",
                bytes_up: route.base.bytes_up,
                bytes_down: route.base.bytes_down,
                uptime_secs: route.base.creation_time.elapsed().as_secs_f64(),
                additional_info: format!("got destroy, reason {reason}"),
            });
        }
        if let Some(exit) = inner.exit_sockets.remove(&circuit_id) {
            // `remove_exit_socket` : purge `intro_point_for` (avec
            // `pex.stop_announce`/dechargement du store) et
            // `rendezvous_point_for` attaches a cette sortie.
            cleanup_exit_socket(&mut inner, circuit_id);
            events.push(CircuitRemovedEvent {
                circuit_id,
                circuit_class: "TunnelExitSocket",
                bytes_up: 0,
                bytes_down: 0,
                uptime_secs: exit.creation_time.elapsed().as_secs_f64(),
                additional_info: format!("got destroy, reason {reason}"),
            });
        }
        if let Some(circuit) = inner.circuits.remove(&circuit_id) {
            events.push(CircuitRemovedEvent {
                circuit_id,
                circuit_class: "Circuit",
                bytes_up: circuit.base.bytes_up,
                bytes_down: circuit.base.bytes_down,
                uptime_secs: circuit.base.creation_time.elapsed().as_secs_f64(),
                additional_info: format!("got destroy, reason {reason}"),
            });
        }
        drop(inner);
        self.notify_circuits_changed();
        for ev in events {
            self.emit_circuit_removed(ev);
        }
        tracing::debug!(circuit_id, reason, "objet de routage detruit");
    }

    /// `on_ping` : repond `pong` via `send_cell` (chiffrement selon
    /// le role local pour ce circuit — BACKWARD cote sortie).
    fn on_tunnel_ping(self: &Arc<Self>, src: SocketAddr, circuit_id: u32, identifier: u16) {
        let known = {
            let inner = self.inner.lock().unwrap();
            inner.circuits.contains_key(&circuit_id)
                || inner.exit_sockets.contains_key(&circuit_id)
                || inner.relays.contains_key(&circuit_id)
        };
        if !known {
            return;
        }
        let pong = tp::TunnelPing {
            circuit_id,
            identifier,
        };
        let addr = UdpAddress::from(src);
        let c = self.clone();
        tokio::spawn(async move {
            let _ = c.send_cell(&addr, &pong).await;
        });
    }

    /// `send_destroy` : paquet signe `DestroyPayload`.
    pub async fn send_destroy(
        &self,
        target: &UdpAddress,
        circuit_id: u32,
        reason: u16,
    ) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        tp::Destroy { circuit_id, reason }.pack(&mut w)?;
        // `ezr_pack` pyipv8 (`send_destroy`) : signe sans `dist`.
        let pkt =
            Packet::sign_no_dist(&self.community_id, msg::DESTROY, &self.key, &w.into_bytes());
        self.endpoint.send_to(target, &pkt).await
    }

    /// `send_data` : envoie des donnees sur un circuit pret.
    pub async fn send_data(
        self: &Arc<Self>,
        circuit_id: u32,
        dest: &UdpAddress,
        origin: &UdpAddress,
        data: &[u8],
    ) -> Result<(), Ipv8Error> {
        let addr = {
            let inner = self.inner.lock().unwrap();
            inner
                .circuits
                .get(&circuit_id)
                .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
        };
        let Some(addr) = addr else {
            return Err(Ipv8Error::Malformed("circuit absent ou sans hop"));
        };
        tracing::trace!(circuit_id, ?addr, "send_data vers premier hop");
        let p = tp::Data {
            circuit_id,
            dest_address: dest.clone(),
            org_address: origin.clone(),
            data: data.to_vec(),
        };
        self.send_cell(&addr, &p).await.map(|_| ())
    }

    /// `send_ping` : cellule ping sur un circuit connu.
    pub async fn send_ping(&self, circuit_id: u32) -> Result<(), Ipv8Error> {
        let addr = {
            let inner = self.inner.lock().unwrap();
            inner
                .circuits
                .get(&circuit_id)
                .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                .or_else(|| {
                    inner
                        .exit_sockets
                        .get(&circuit_id)
                        .and_then(|e| e.hop.address.clone())
                })
        };
        let Some(addr) = addr else {
            return Err(Ipv8Error::Malformed("circuit inconnu"));
        };
        let ping = tp::TunnelPing {
            circuit_id,
            identifier: self.next_id(),
        };
        self.send_cell(&addr, &ping).await.map(|_| ())
    }
}

/// `remove_exit_socket` (`hidden_services.py`) : purge les entrees
/// `intro_point_for` rattachees a cette sortie (`stop_announce` dans
/// le store PEX, decharge si `done`) et les `rendezvous_point_for`.
fn cleanup_exit_socket(inner: &mut Inner, circuit_id: u32) {
    let detached: Vec<(Vec<u8>, [u8; 20])> = inner
        .intro_point_for
        .iter()
        .filter(|(_, (cid, _))| *cid == circuit_id)
        .map(|(pk, (_, ih))| (pk.clone(), *ih))
        .collect();
    for (seeder_pk, info_hash) in detached {
        inner.intro_point_for.remove(&seeder_pk);
        if let Some(store) = inner.pex.get_mut(&info_hash) {
            store.stop_announce(&seeder_pk);
            if store.is_done() {
                inner.pex.remove(&info_hash);
            }
        }
    }
    inner
        .rendezvous_point_for
        .retain(|_, cid| *cid != circuit_id);
}

/// `generate_diffie_secret` : nouvelle paire X25519 ephemere —
/// retourne (secret 32o, public 32o).
pub fn generate_diffie_secret() -> ([u8; 32], [u8; 32]) {
    let mut sk = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut sk);
    let secret = StaticSecret::from(sk);
    (sk, X25519PublicKey::from(&secret).to_bytes())
}

/// Resultat de `generate_diffie_shared_secret`.
pub(crate) struct DiffieShared {
    /// Secret partage `s1 || s2` (64 octets).
    pub(crate) shared: [u8; 64],
    /// `crypt_pk` de la cle ephemere repondante (envoye dans `key`).
    pub(crate) crypt_pk: [u8; 32],
    /// `auth` = `crypto_auth(shared[:32], crypt_pk)`.
    pub(crate) auth: [u8; 32],
}

/// `generate_diffie_shared_secret` (hop) : `shared = DH(tmp2, dh) +
/// DH(node_sk, dh)` ; retourne le secret partage, la cle publique
/// ephemere et son auth.
pub(crate) fn generate_diffie_shared_secret(
    dh_received: &[u8],
    node_key: &LibNaClSecretKey,
) -> Result<DiffieShared, Ipv8Error> {
    let (tmp2_sk, tmp2_pk) = generate_diffie_secret();
    let s1 = crypto_box_beforenm(dh_received, &tmp2_sk)?;
    let s2 = crypto_box_beforenm(dh_received, node_key.crypt_x25519().as_bytes())?;
    let mut shared = [0u8; 64];
    shared[..32].copy_from_slice(&s1);
    shared[32..].copy_from_slice(&s2);
    let auth = crypto_auth(&shared[..32], &tmp2_pk)?;
    Ok(DiffieShared {
        shared,
        crypt_pk: tmp2_pk,
        auth,
    })
}

/// `verify_and_generate_shared_secret` (initiateur) :
/// `s1 = DH(tmp, dh_received)`, `s2 = DH(tmp, crypt_pk du hop)` ;
/// verifie `auth`. Retourne le secret partage (64 octets).
pub(crate) fn verify_and_generate_shared_secret(
    dh_secret: &[u8; 32],
    dh_received: &[u8],
    auth: &[u8; 32],
    hop_crypt_pk: &[u8; 32],
) -> Result<[u8; 64], Ipv8Error> {
    let s1 = crypto_box_beforenm(dh_received, dh_secret)?;
    let s2 = crypto_box_beforenm(hop_crypt_pk, dh_secret)?;
    let mut shared = [0u8; 64];
    shared[..32].copy_from_slice(&s1);
    shared[32..].copy_from_slice(&s2);
    if !crypto_auth_verify(auth, &shared[..32], dh_received) {
        return Err(Ipv8Error::Crypto(tribler_crypto::CryptoError::Aead));
    }
    Ok(shared)
}
