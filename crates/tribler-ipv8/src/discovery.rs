//! `DiscoveryCommunity` (equivalent de `peerdiscovery/community.py`) :
//! bootstrap + marche aleatoire via similarity-request + ping/pong +
//! introduction-request/response (ancien format IPv4 — suffisant pour
//! l'etape 9 ; le format "new style" `ip_address` arrive a l'etape 11).
//!
//! `community_id` = `7e313685c1912a141279f8248fc8db5899c5df5a`
//! (identique au Python).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;

use crate::address::UdpAddress;
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet};
use crate::payloads::{
    msg, ConnectionType, IntroductionRequest, IntroductionResponse, Payload, Ping,
    SimilarityRequest, SimilarityResponse,
};
use crate::peer::{Network, Peer};
use crate::serializer::Writer;
use crate::CommunityId;

/// `community_id` de la `DiscoveryCommunity` pyipv8 (inchange).
pub const DISCOVERY_COMMUNITY_ID: CommunityId = [
    0x7e, 0x31, 0x36, 0x85, 0xc1, 0x91, 0x2a, 0x14, 0x12, 0x79, 0xf8, 0x24, 0x8f, 0xc8, 0xdb, 0x58,
    0x99, 0xc5, 0xdf, 0x5a,
];

/// Intervalle entre deux etapes de marche aleatoire
/// (`RandomWalk`/`take_step` : le Python utilise des strategies a
/// intervalle configurable ; 25 s est l'ordre de grandeur historique).
const WALK_INTERVAL: Duration = Duration::from_secs(25);

/// Nombre maximum de pairs connus avant de couper les introductions
/// (`max_peers` Python par defaut cote strategy).
const DEFAULT_MAX_PEERS: usize = 30;

/// Lamport clock locale (compteur monotone par community).
fn now_global_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Community de decouverte minimale.
pub struct DiscoveryCommunity {
    /// Identite locale.
    key: LibNaClSecretKey,
    /// Annuaire reseau partage.
    network: Arc<Network>,
    /// Endpoint UDP partage.
    endpoint: Arc<UdpEndpoint>,
    /// Nonce courant pour ping/introduction (mod 65536).
    identifier: std::sync::atomic::AtomicU16,
    /// Requetes d'introduction en attente (identifier -> instant).
    pending_intro: std::sync::Mutex<std::collections::HashMap<u16, (UdpAddress, Instant)>>,
}

impl DiscoveryCommunity {
    /// Cree la community et s'enregistre aupres de l'endpoint.
    pub async fn new(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
    ) -> Arc<Self> {
        let community = Arc::new(Self {
            key,
            network,
            endpoint: endpoint.clone(),
            identifier: std::sync::atomic::AtomicU16::new(0),
            pending_intro: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        let prefix = prefix_of(&DISCOVERY_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(prefix, Arc::new(move |src, pkt| c.on_packet(src, pkt)))
            .await;
        community
    }

    /// Prochain identifiant de requete (mod 65536 comme le Python —
    /// `fetch_add` sur `u16` boucle naturellement).
    fn next_id(&self) -> u16 {
        self.identifier
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Serialise + signe + envoie un payload a une adresse.
    async fn send_payload<P: Payload>(
        &self,
        addr: &UdpAddress,
        payload: &P,
    ) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        payload.pack(&mut w)?;
        let packet = Packet::sign(
            &DISCOVERY_COMMUNITY_ID,
            P::MSG_ID,
            &self.key,
            now_global_time() % 65536,
            &w.into_bytes(),
        );
        self.endpoint.send_to(addr, &packet).await
    }

    /// `on_packet` : dispatch par `msg_id` (cf. `decode_map` Python).
    fn on_packet(&self, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        // Le pair est "verifie" (signature OK deja controlee par
        // `Packet::parse`) — enregistre dans l'annuaire.
        let peer = Peer::new(pkt.public_key_bin.clone(), Some(UdpAddress::from(src)));
        if let Some(p) = &peer {
            self.network.add_verified(p.clone());
            self.network
                .discover_service(&pkt.public_key_bin, DISCOVERY_COMMUNITY_ID);
        }

        let mut r = crate::serializer::Reader::new(&pkt.payload);
        match pkt.msg_id {
            msg::PING => {
                let p = Ping::unpack(&mut r)?;
                let ep = self.endpoint.clone();
                let key = self.key.clone();
                let addr = UdpAddress::from(src);
                let mut w = Writer::new();
                w.u16(p.identifier);
                let packet = Packet::sign(
                    &DISCOVERY_COMMUNITY_ID,
                    msg::PONG,
                    &key,
                    now_global_time() % 65536,
                    &w.into_bytes(),
                );
                tokio::spawn(async move {
                    let _ = ep.send_to(&addr, &packet).await;
                });
            }
            msg::SIMILARITY_REQUEST => {
                let p = SimilarityRequest::unpack(&mut r)?;
                // Repond par la liste des services connus du demandeur
                // (ici : la discovery community elle-meme).
                let resp = SimilarityResponse {
                    identifier: p.identifier,
                    preference_list: vec![DISCOVERY_COMMUNITY_ID],
                    tb_overlap: vec![(DISCOVERY_COMMUNITY_ID, 0)],
                };
                let ep = self.endpoint.clone();
                let key = self.key.clone();
                let addr = UdpAddress::from(src);
                tokio::spawn(async move {
                    let mut w = Writer::new();
                    if resp.pack(&mut w).is_ok() {
                        let packet = Packet::sign(
                            &DISCOVERY_COMMUNITY_ID,
                            msg::SIMILARITY_RESPONSE,
                            &key,
                            now_global_time() % 65536,
                            &w.into_bytes(),
                        );
                        let _ = ep.send_to(&addr, &packet).await;
                    }
                });
            }
            msg::INTRODUCTION_REQUEST => {
                let addr = UdpAddress::from(src);
                let p = IntroductionRequest::unpack(&mut r)?;
                let ep = self.endpoint.clone();
                let key = self.key.clone();
                let my_lan = guess_lan(src);
                let my_wan = UdpAddress::from(src);
                // Pair introduit : le premier autre pair connu (simple).
                let intro = self
                    .network
                    .peers_for_service(&DISCOVERY_COMMUNITY_ID)
                    .into_iter()
                    .find(|q| q.public_key_bin != pkt.public_key_bin)
                    .and_then(|q| q.address);
                let (lan_i, wan_i) = match &intro {
                    Some(a) => (
                        UdpAddress::from(a.to_socket_addr().unwrap_or(src)),
                        a.clone(),
                    ),
                    None => (
                        UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                            std::net::Ipv4Addr::UNSPECIFIED,
                            0,
                        )),
                        UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                            std::net::Ipv4Addr::UNSPECIFIED,
                            0,
                        )),
                    ),
                };
                let resp = IntroductionResponse {
                    destination_address: p.source_wan_address.clone(),
                    source_lan_address: my_lan,
                    source_wan_address: my_wan,
                    lan_introduction_address: lan_i,
                    wan_introduction_address: wan_i,
                    connection_type: ConnectionType::Unknown,
                    supports_new_style: false,
                    intro_supports_new_style: false,
                    peer_limit_reached: self.network.len() >= DEFAULT_MAX_PEERS,
                    identifier: p.identifier,
                    extra_bytes: Vec::new(),
                };
                tokio::spawn(async move {
                    let mut w = Writer::new();
                    if resp.pack(&mut w).is_ok() {
                        let packet = Packet::sign(
                            &DISCOVERY_COMMUNITY_ID,
                            msg::INTRODUCTION_RESPONSE,
                            &key,
                            now_global_time() % 65536,
                            &w.into_bytes(),
                        );
                        let _ = ep.send_to(&addr, &packet).await;
                    }
                });
            }
            _ => {
                tracing::trace!(msg_id = pkt.msg_id, "message discovery ignore");
            }
        }
        Ok(())
    }

    /// Envoie un `ping` (msg 3) — equivalent de `send_ping` Python.
    pub async fn send_ping(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        self.send_payload(addr, &Ping { identifier: id }).await?;
        Ok(id)
    }

    /// Envoie une `similarity-request` (msg 1) — coeur de la marche
    /// aleatoire (`RandomWalk.take_step`).
    pub async fn send_similarity_request(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        let lan = guess_lan_for(addr);
        let wan = addr.clone();
        let p = SimilarityRequest {
            identifier: id,
            lan_address: lan,
            wan_address: wan,
            connection_type: ConnectionType::Unknown,
            preference_list: vec![DISCOVERY_COMMUNITY_ID],
        };
        self.send_payload(addr, &p).await?;
        Ok(id)
    }

    /// Envoie une `introduction-request` (msg 246).
    pub async fn send_introduction_request(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        let lan = guess_lan_for(addr);
        let wan = addr.clone();
        let p = IntroductionRequest {
            destination_address: addr.clone(),
            source_lan_address: lan,
            source_wan_address: wan,
            advice: true,
            supports_new_style: false,
            connection_type: ConnectionType::Unknown,
            identifier: id,
            extra_bytes: Vec::new(),
        };
        self.send_payload(addr, &p).await?;
        self.pending_intro
            .lock()
            .unwrap()
            .insert(id, (addr.clone(), Instant::now()));
        Ok(id)
    }

    /// Une etape de marche aleatoire : envoie similarity-request a un
    /// pair connu (ou a un bootstrap si aucun). Equivalent de
    /// `RandomChurn`/`take_step`.
    pub async fn step(&self, bootstrap: &[UdpAddress]) -> Result<(), Ipv8Error> {
        let peers = self.network.peers_for_service(&DISCOVERY_COMMUNITY_ID);
        if let Some(p) = peers.first() {
            if let Some(addr) = &p.address {
                self.send_similarity_request(addr).await?;
                return Ok(());
            }
        }
        if let Some(addr) = bootstrap.first() {
            self.send_introduction_request(addr).await?;
        }
        Ok(())
    }

    /// Boucle de marche aleatoire (a spawner).
    pub async fn run(self: &Arc<Self>, bootstrap: Vec<UdpAddress>) {
        let mut tick = tokio::time::interval(WALK_INTERVAL);
        loop {
            tick.tick().await;
            if let Err(e) = self.step(&bootstrap).await {
                tracing::debug!(error = %e, "etape de marche discovery echouee");
            }
        }
    }

    /// Nombre de pairs verifies connus.
    pub fn peer_count(&self) -> usize {
        self.network.len()
    }
}

/// Estime l'adresse LAN a partir de l'adresse source vue (meme
/// heuristique simplifiee que le Python : adresse locale du socket si
/// non resoluble).
fn guess_lan(_src: SocketAddr) -> UdpAddress {
    // Approximation : on renseigne 0.0.0.0:0 si on ne connait pas
    // mieux — le Python fait une resolution d'interface reelle
    // (`interfaces/lan_addresses`), reportee a l'etape 11.
    UdpAddress::Ipv4(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::UNSPECIFIED,
        0,
    ))
}

/// Variante pour l'emission : meme heuristique simplifiee.
fn guess_lan_for(_dst: &UdpAddress) -> UdpAddress {
    UdpAddress::Ipv4(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::UNSPECIFIED,
        0,
    ))
}
