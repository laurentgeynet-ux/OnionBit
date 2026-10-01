// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `DiscoveryCommunity` (equivalent de `peerdiscovery/community.py` +
//! handlers generiques de `community.py`) : bootstrap + marche
//! aleatoire via similarity-request + ping/pong +
//! introduction-request/response (formats **ancien IPv4** et **nouveau
//! `ip_address`**), gestion des adresses "walkable" introduites,
//! puncture/puncture-request, horloge de Lamport.
//!
//! `community_id` = `7e313685c1912a141279f8248fc8db5899c5df5a`
//! (identique au Python).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use rand::seq::SliceRandom;

use crate::address::UdpAddress;
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet};
use crate::payloads::{
    msg, ConnectionType, IntroductionRequest, IntroductionResponse, NewIntroductionRequest,
    NewIntroductionResponse, Payload, Ping, SimilarityRequest, SimilarityResponse,
};
use crate::peer::{Network, Peer};
use crate::serializer::{Reader, Writer};
use crate::CommunityId;

/// `community_id` de la `DiscoveryCommunity` pyipv8 (inchange).
pub const DISCOVERY_COMMUNITY_ID: CommunityId = [
    0x7e, 0x31, 0x36, 0x85, 0xc1, 0x91, 0x2a, 0x14, 0x12, 0x79, 0xf8, 0x24, 0x8f, 0xc8, 0xdb, 0x58,
    0x99, 0xc5, 0xdf, 0x5a,
];

/// Nombre maximum de pairs connus avant de couper les introductions
/// (`max_peers` Python par defaut cote strategy).
const DEFAULT_MAX_PEERS: usize = 30;

/// Probabilite de re-bootstrap quand des pairs existent
/// (`random() < 0.05` Python dans `get_new_introduction`).
const REBOOTSTRAP_CHANCE: f64 = 0.05;

/// `RandomWalk.node_timeout` Python : une adresse sans reponse apres
/// ce delai est oubliee (3 s par defaut).
const INTRO_TIMEOUT: Duration = Duration::from_secs(3);
/// `RandomWalk.window_size` Python : nombre maximal d'introductions
/// en vol avant de suspendre la marche (5 par defaut).
const INTRO_WINDOW: usize = 5;
/// `RandomWalk.reset_chance` Python : chance sur 255 de sauter le
/// walkable et de demander une nouvelle introduction a un pair connu
/// (50 → ~80 % des etapes explorent une adresse introduite).
const RESET_CHANCE: u8 = 50;

/// `RandomChurn.sample_size` Python : nombre de pairs verifies
/// echantillonnes par etape de churn.
const CHURN_SAMPLE_SIZE: usize = 8;
/// `RandomChurn.ping_interval` Python : delai minimal entre deux
/// pings de vivacite vers une meme adresse (10 s par defaut).
const CHURN_PING_INTERVAL: Duration = Duration::from_millis(10_000);
/// `RandomChurn.inactive_time` Python : un pair sans reponse depuis
/// ce delai est pingue (27,5 s par defaut).
const CHURN_INACTIVE: Duration = Duration::from_millis(27_500);
/// `RandomChurn.drop_time` Python : un pair pingue toujours sans
/// reponse apres ce delai total est supprime de l'annuaire (57,5 s
/// par defaut).
const CHURN_DROP: Duration = Duration::from_millis(57_500);

/// Adresse "non definie" (`0.0.0.0:0`).
fn unspecified() -> UdpAddress {
    UdpAddress::unspecified()
}

/// `true` si l'IPv4 est dans un sous-reseau LAN prive
/// (`address_in_lan_subnets` Python : 10/8, 172.16/12, 192.168/16,
/// 127/8, 169.254/16).
pub fn is_lan_subnet(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || o[0] == 127
        || (o[0] == 169 && o[1] == 254)
}

/// Community de decouverte (overlay generique : marche aleatoire +
/// puncture + Lamport — les autres communities reutilisent les memes
/// primitives via `packet`/`payloads`).
pub struct DiscoveryCommunity {
    /// Identite locale.
    key: LibNaClSecretKey,
    /// Annuaire reseau partage.
    network: Arc<Network>,
    /// Endpoint UDP partage.
    endpoint: Arc<UdpEndpoint>,
    /// `Weak` pour respawner depuis les handlers synchrones.
    weak: std::sync::Weak<Self>,
    /// Nonce courant pour ping/introduction (mod 65536).
    identifier: std::sync::atomic::AtomicU16,
    /// Horloge de Lamport (`my_peer.lamport_timestamp` de l'overlay).
    global_time: AtomicU64,
    /// `my_estimated_wan` Python (appris via introduction-response).
    my_estimated_wan: Mutex<UdpAddress>,
    /// `my_estimated_lan`.
    my_estimated_lan: Mutex<UdpAddress>,
    /// Requetes d'introduction en attente (identifier -> instant).
    pending_intro: Mutex<std::collections::HashMap<u16, (UdpAddress, Instant)>>,
    /// `intro_timeouts` de `RandomWalk` : adresses walkable vers
    /// lesquelles une introduction est en vol (adresse -> instant).
    intro_timeouts: Mutex<std::collections::HashMap<UdpAddress, Instant>>,
    /// Adresses ajoutees dynamiquement au bootstrap
    /// (`DispersyBootstrapper.ip_addresses.append` — endpoint REST
    /// `/api/ipv8/isolation` "bootstrapnode").
    extra_bootstrap: Mutex<Vec<UdpAddress>>,
    /// `RandomChurn._pinged` Python : adresses pingees pour verifier
    /// leur vivacite (adresse -> instant du dernier ping de churn).
    churn_pinged: Mutex<std::collections::HashMap<UdpAddress, Instant>>,
    /// Observables de decode (equivalents des hooks pyipv8
    /// `introduction_request/response_callback` et `on_puncture`) —
    /// utilises par les tests et le banc d'interop.
    intro_requests_seen: std::sync::atomic::AtomicUsize,
    intro_responses_seen: std::sync::atomic::AtomicUsize,
    punctures_seen: std::sync::atomic::AtomicUsize,
}

impl DiscoveryCommunity {
    /// Cree la community et s'enregistre aupres de l'endpoint.
    /// `my_lan` : adresse LAN estimee (`UdpAddress::Ipv4(0.0.0.0:0)`
    /// si inconnue — le Python fait une resolution d'interface reelle,
    /// reportee).
    pub async fn new(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
        my_lan: UdpAddress,
    ) -> Arc<Self> {
        let community = Arc::new_cyclic(|weak| Self {
            key,
            network,
            endpoint: endpoint.clone(),
            weak: weak.clone(),
            identifier: std::sync::atomic::AtomicU16::new(0),
            global_time: AtomicU64::new(0),
            my_estimated_wan: Mutex::new(unspecified()),
            my_estimated_lan: Mutex::new(my_lan),
            pending_intro: Mutex::new(std::collections::HashMap::new()),
            intro_timeouts: Mutex::new(std::collections::HashMap::new()),
            extra_bootstrap: Mutex::new(Vec::new()),
            churn_pinged: Mutex::new(std::collections::HashMap::new()),
            intro_requests_seen: std::sync::atomic::AtomicUsize::new(0),
            intro_responses_seen: std::sync::atomic::AtomicUsize::new(0),
            punctures_seen: std::sync::atomic::AtomicUsize::new(0),
        });
        let prefix = prefix_of(&DISCOVERY_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(
                prefix,
                Arc::new(move |src, pkt| c.on_packet(src, pkt)),
                crate::packet::WIRE_DISCOVERY,
            )
            .await;
        community
    }

    /// `global_time` (lecture seule — tests et observabilite).
    pub fn global_time(&self) -> u64 {
        self.global_time.load(Ordering::Relaxed)
    }

    /// `claim_global_time` : incremente et retourne l'horodatage.
    fn claim_global_time(&self) -> u64 {
        self.global_time.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// `update_global_time` : l'horodatage recu fait avancer l'horloge.
    fn update_global_time(&self, t: u64) {
        self.global_time.fetch_max(t, Ordering::Relaxed);
    }

    /// Prochain identifiant de requete (mod 65536 comme le Python —
    /// `fetch_add` sur `u16` boucle naturellement).
    fn next_id(&self) -> u16 {
        self.identifier.fetch_add(1, Ordering::Relaxed)
    }

    /// `my_estimated_wan`.
    pub fn my_estimated_wan(&self) -> UdpAddress {
        self.my_estimated_wan.lock().unwrap().clone()
    }

    /// Injecte une estimation WAN initiale (`my_estimated_wan` Python
    /// est aussi assignable en test). Utile sur un banc 100 % loopback :
    /// `destination_address` d'une intro-response en 127/8 n'est jamais
    /// retenue comme WAN (`address_in_lan_subnets`), ce qui laisse le
    /// DHT muet (`on_node_discovered` refuse tout noeud sans WAN).
    pub fn set_estimated_wan(&self, wan: UdpAddress) {
        *self.my_estimated_wan.lock().unwrap() = wan;
    }

    /// `my_estimated_lan`.
    pub fn my_estimated_lan(&self) -> UdpAddress {
        self.my_estimated_lan.lock().unwrap().clone()
    }

    /// `overlay.walk_to(address)` (`RandomWalk.walk_to` →
    /// introduction-request vers une adresse explicite).
    pub async fn walk_to(&self, addr: &UdpAddress) -> Result<(), Ipv8Error> {
        self.send_introduction_request(addr).await.map(|_| ())
    }

    /// `bootstrapper.ip_addresses.append(...)` : ajoute une adresse
    /// au pool de bootstrap (consultee par `step`/`run`).
    pub fn add_bootstrapper(&self, addr: UdpAddress) {
        self.extra_bootstrap.lock().unwrap().push(addr);
    }

    /// Pool de bootstrap effectif : liste initiale + ajouts
    /// dynamiques (`extra_bootstrap`).
    fn merged_bootstrap(&self, bootstrap: &[UdpAddress]) -> Vec<UdpAddress> {
        let mut v = bootstrap.to_vec();
        v.extend(self.extra_bootstrap.lock().unwrap().iter().cloned());
        v
    }

    /// `OverlaySchema` : instantane REST de la community
    /// (`GET /api/ipv8/overlays`).
    pub fn overlay_info(&self, is_isolated: bool) -> crate::overlays::OverlayInfo {
        use crate::overlays::{
            discovery_msg_name, overlay_peer, OverlayInfo, OverlayStrategy, DEFAULT_MAX_PEERS,
        };
        OverlayInfo {
            community_id: DISCOVERY_COMMUNITY_ID,
            my_peer_hex: hex::encode(self.key.public_key().to_bin()),
            global_time: self.global_time(),
            peers: self
                .network
                .peers_for_service(&DISCOVERY_COMMUNITY_ID)
                .iter()
                .map(overlay_peer)
                .collect(),
            overlay_name: "DiscoveryCommunity",
            max_peers: DEFAULT_MAX_PEERS,
            is_isolated,
            my_estimated_wan: self.my_estimated_wan(),
            my_estimated_lan: self.my_estimated_lan(),
            // Configuration `ipv8_default_config` pyipv8 pour
            // `DiscoveryCommunity`.
            strategies: vec![
                OverlayStrategy {
                    name: "RandomWalk",
                    target_peers: 20,
                },
                OverlayStrategy {
                    name: "RandomChurn",
                    target_peers: -1,
                },
                OverlayStrategy {
                    name: "PeriodicSimilarity",
                    target_peers: -1,
                },
            ],
            decode: discovery_msg_name,
        }
    }

    /// Serialise + signe + envoie un payload (global_time mod 65536
    /// comme les introductions Python — `_ez_pack` prend la valeur
    /// brute mais les champs `identifier`/`global_time` sont tires
    /// modulo 2^16 dans `create_introduction_*`).
    async fn send_payload<P: Payload>(
        &self,
        addr: &UdpAddress,
        payload: &P,
    ) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        payload.pack(&mut w)?;
        let gtime = self.claim_global_time() % 65536;
        let body = w.into_bytes();
        // `lazy_wrapper_unsigned` Python : ping/pong (3/4) et
        // puncture-requests sont non signes (`_ez_pack(..., sig=False)`)
        // ; tout le reste de la community est signe avec `dist`.
        let packet = if crate::packet::WIRE_DISCOVERY.is_unsigned(P::MSG_ID) {
            Packet::pack_unsigned(&DISCOVERY_COMMUNITY_ID, P::MSG_ID, gtime, &body)
        } else {
            Packet::sign(&DISCOVERY_COMMUNITY_ID, P::MSG_ID, &self.key, gtime, &body)
        };
        self.endpoint.send_to(addr, &packet).await
    }

    /// `on_packet` : dispatch par `msg_id` (cf. `decode_map` Python).
    fn on_packet(self: &Arc<Self>, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        let src_addr = UdpAddress::from(src);

        // `Community.on_packet` pyipv8 : tout paquet recu d'un pair
        // verifie prouve qu'il est vivant — `last_response` rafraichi
        // avant tout traitement (sert a `RandomChurn`/`is_inactive`).
        self.network.touch_by_addr(&src);

        // Messages non signes (`lazy_wrapper_unsigned` — avec `dist`
        // mais sans auth) : ping/pong (3/4) et puncture-request
        // (250/232).
        if !pkt.signed {
            return match pkt.msg_id {
                msg::PING => {
                    self.update_global_time(pkt.global_time);
                    let mut r = Reader::new(&pkt.payload);
                    let p = Ping::unpack(&mut r)?;
                    let c = self.clone();
                    tokio::spawn(async move {
                        let _ = c
                            .send_payload(
                                &src_addr,
                                &crate::payloads::Pong {
                                    identifier: p.identifier,
                                },
                            )
                            .await;
                    });
                    Ok(())
                }
                msg::PONG => {
                    self.update_global_time(pkt.global_time);
                    Ok(())
                }
                _ => self.on_puncture_request(src_addr, &pkt),
            };
        }

        // `update_global_time` avant tout (horloge de Lamport).
        self.update_global_time(pkt.global_time);

        let peer = Peer::new(pkt.public_key_bin.clone(), Some(src_addr.clone()));
        if let Some(p) = &peer {
            self.network.add_verified(p.clone());
            self.network
                .discover_service(&pkt.public_key_bin, DISCOVERY_COMMUNITY_ID);
        }

        let mut r = Reader::new(&pkt.payload);
        match pkt.msg_id {
            msg::PING => {
                let p = Ping::unpack(&mut r)?;
                let c = self.clone();
                tokio::spawn(async move {
                    let _ = c
                        .send_payload(
                            &src_addr,
                            &crate::payloads::Pong {
                                identifier: p.identifier,
                            },
                        )
                        .await;
                });
            }
            msg::PONG => {
                // `on_pong` : le pair est deja verifie (signature OK) —
                // Python met a jour `last_response` ; rien d'autre.
            }
            msg::SIMILARITY_REQUEST => {
                let p = SimilarityRequest::unpack(&mut r)?;
                // Repond par les services connus du demandeur.
                let resp = SimilarityResponse {
                    identifier: p.identifier,
                    preference_list: vec![DISCOVERY_COMMUNITY_ID],
                    tb_overlap: vec![(DISCOVERY_COMMUNITY_ID, 0)],
                };
                let c = self.clone();
                tokio::spawn(async move {
                    let _ = c.send_payload(&src_addr, &resp).await;
                });
            }
            msg::INTRODUCTION_REQUEST => {
                let p = IntroductionRequest::unpack(&mut r)?;
                self.on_introduction_request(peer.clone(), &src_addr, IntroFields::from(&p), false);
            }
            msg::NEW_INTRODUCTION_REQUEST => {
                let p = NewIntroductionRequest::unpack(&mut r)?;
                // `peer.new_style_intro = True` avant traitement.
                let mut peer = peer;
                if let Some(ref mut p) = peer {
                    p.new_style_intro = true;
                }
                self.on_introduction_request(peer, &src_addr, IntroFields::from(&p), true);
            }
            msg::INTRODUCTION_RESPONSE => {
                let p = IntroductionResponse::unpack(&mut r)?;
                let mut peer = peer;
                if let Some(ref mut pr) = peer {
                    pr.new_style_intro = p.supports_new_style;
                }
                self.on_introduction_response(
                    &src_addr,
                    IntroResponseFields::from(&p),
                    peer,
                    false,
                );
            }
            msg::NEW_INTRODUCTION_RESPONSE => {
                let p = NewIntroductionResponse::unpack(&mut r)?;
                let mut peer = peer;
                if let Some(ref mut pr) = peer {
                    pr.new_style_intro = true;
                }
                self.on_introduction_response(&src_addr, IntroResponseFields::from(&p), peer, true);
            }
            msg::PUNCTURE | msg::NEW_PUNCTURE => {
                // `on_puncture` Python : no-op (le trou NAT est ouvert
                // par la reception meme) — on compte juste le decode.
                self.punctures_seen
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            _ => {
                tracing::trace!(msg_id = pkt.msg_id, "message discovery ignore");
            }
        }
        Ok(())
    }

    /// Corps de `on_introduction_request` Python (commun aux formats
    /// ancien/nouveau).
    fn on_introduction_request(
        self: &Arc<Self>,
        peer: Option<Peer>,
        src_addr: &UdpAddress,
        req: IntroFields,
        new_style: bool,
    ) {
        let Some(peer) = peer else { return };
        if self.network.len() >= DEFAULT_MAX_PEERS {
            tracing::debug!("introduction-request ignoree : trop de pairs");
            return;
        }
        self.intro_requests_seen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // L'adresse LAN annoncee devient l'adresse preferee si c'est
        // une IPv4 (`peer.address = UDPv4LANAddress(...)` Python —
        // simplifie : on conserve l'adresse source vue).
        self.network.add_verified(peer.clone());
        self.network
            .discover_service(&peer.public_key_bin, DISCOVERY_COMMUNITY_ID);

        // Choix du pair a introduire (`get_peer_for_introduction`).
        let intro_peer = self
            .network
            .peers_for_service(&DISCOVERY_COMMUNITY_ID)
            .into_iter()
            .filter(|q| q.public_key_bin != peer.public_key_bin)
            .filter(|q| q.address.as_ref().is_some_and(|a| !a.is_unspecified()))
            .collect::<Vec<_>>();
        let introduction = intro_peer.choose(&mut rand::thread_rng()).cloned();
        let (lan_i, wan_i) = match introduction.as_ref().and_then(|p| p.address.clone()) {
            Some(a) => (a.clone(), a),
            None => (unspecified(), unspecified()),
        };

        // Reponse : nouveau format si le demandeur le supporte.
        let c = self.clone();
        let dst = src_addr.clone();
        let id = req.identifier;
        let my_lan = self.my_estimated_lan();
        let my_wan = if self.my_estimated_wan().is_unspecified() {
            src_addr.clone()
        } else {
            self.my_estimated_wan()
        };
        let dest = req.source_wan_address.clone();
        let limit = self.network.len() >= DEFAULT_MAX_PEERS;
        let intro_new_style = introduction
            .as_ref()
            .map(|p| p.new_style_intro)
            .unwrap_or(false);
        tokio::spawn(async move {
            let mut w = Writer::new();
            let res = if new_style {
                NewIntroductionResponse {
                    destination_address: dest,
                    source_lan_address: my_lan,
                    source_wan_address: my_wan,
                    lan_introduction_address: lan_i,
                    wan_introduction_address: wan_i,
                    identifier: id,
                    intro_supports_new_style: intro_new_style,
                    extra_bytes: Vec::new(),
                }
                .pack(&mut w)
            } else {
                IntroductionResponse {
                    destination_address: dest,
                    source_lan_address: my_lan,
                    source_wan_address: my_wan,
                    lan_introduction_address: lan_i.clone(),
                    wan_introduction_address: wan_i.clone(),
                    connection_type: ConnectionType::Unknown,
                    supports_new_style: true,
                    peer_limit_reached: limit,
                    identifier: id,
                    intro_supports_new_style: intro_new_style,
                    extra_bytes: Vec::new(),
                }
                .pack(&mut w)
            };
            if res.is_ok() {
                let pkt = Packet::sign(
                    &DISCOVERY_COMMUNITY_ID,
                    if new_style {
                        msg::NEW_INTRODUCTION_RESPONSE
                    } else {
                        msg::INTRODUCTION_RESPONSE
                    },
                    &c.key,
                    c.claim_global_time() % 65536,
                    &w.into_bytes(),
                );
                let _ = c.endpoint.send_to(&dst, &pkt).await;
            }
        });

        // Si un pair a ete introduit : puncture-request vers lui pour
        // qu'il ouvre son NAT vers le demandeur
        // (`create_introduction_response` Python).
        if let Some(intro) = introduction {
            if let Some(intro_addr) = intro.address {
                let req_lan = req.source_lan_address.clone();
                let req_wan = req.source_wan_address.clone();
                let c = self.clone();
                tokio::spawn(async move {
                    let pkt = c.make_puncture_request(&req_lan, &req_wan, id, new_style);
                    let _ = c.endpoint.send_to(&intro_addr, &pkt).await;
                });
            }
        }
    }

    /// Corps de `on_introduction_response` Python : met a jour
    /// `my_estimated_wan`, enregistre le pair et les adresses
    /// introduites comme "walkable".
    fn on_introduction_response(
        &self,
        src_addr: &UdpAddress,
        resp: IntroResponseFields,
        peer: Option<Peer>,
        _new_style: bool,
    ) {
        self.intro_responses_seen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // `my_estimated_wan` = destination_address si elle n'est pas
        // dans un sous-reseau LAN (`address_in_lan_subnets` Python).
        if let UdpAddress::Ipv4(d) = &resp.destination_address {
            if !is_lan_subnet(*d.ip()) {
                *self.my_estimated_wan.lock().unwrap() = UdpAddress::Ipv4(*d);
            }
        } else if !resp.destination_address.is_unspecified() {
            *self.my_estimated_wan.lock().unwrap() = resp.destination_address.clone();
        }

        if let Some(peer) = peer {
            self.network.add_verified(peer.clone());
            self.network
                .discover_service(&peer.public_key_bin, DISCOVERY_COMMUNITY_ID);

            // Selection des introductions (`introductions` Python) :
            // - WAN != notre WAN : on accepte LAN + WAN ;
            // - WAN == notre WAN : LAN seul ;
            // - sinon : WAN + (notre LAN ip, port WAN).
            let my_wan = self.my_estimated_wan();
            let same_wan_ip = |a: &UdpAddress| match (a, &my_wan) {
                (UdpAddress::Ipv4(x), UdpAddress::Ipv4(y)) => x.ip() == y.ip(),
                (UdpAddress::Ipv6(x), UdpAddress::Ipv6(y)) => x.ip() == y.ip(),
                _ => false,
            };
            let mut introductions = Vec::new();
            if !resp.wan_introduction_address.is_unspecified()
                && !same_wan_ip(&resp.wan_introduction_address)
            {
                if !resp.lan_introduction_address.is_unspecified() {
                    introductions.push(resp.lan_introduction_address.clone());
                }
                introductions.push(resp.wan_introduction_address.clone());
            } else if !resp.lan_introduction_address.is_unspecified()
                && same_wan_ip(&resp.wan_introduction_address)
            {
                introductions.push(resp.lan_introduction_address.clone());
            } else if !resp.wan_introduction_address.is_unspecified() {
                introductions.push(resp.wan_introduction_address.clone());
                if let (UdpAddress::Ipv4(wan_i), UdpAddress::Ipv4(lan)) =
                    (&resp.wan_introduction_address, &my_wan)
                {
                    // Meme WAN : on tente (ip LAN, port WAN).
                    let my_lan = self.my_estimated_lan();
                    if let UdpAddress::Ipv4(my_lan_v4) = my_lan {
                        introductions.push(UdpAddress::Ipv4(std::net::SocketAddrV4::new(
                            *my_lan_v4.ip(),
                            wan_i.port(),
                        )));
                        let _ = lan;
                    }
                }
            }
            for addr in introductions {
                self.network.discover_address(
                    &peer,
                    addr,
                    Some(DISCOVERY_COMMUNITY_ID),
                    resp.intro_supports_new_style,
                );
            }
        }
        let _ = src_addr;
    }

    /// `on_puncture_request` : envoie un puncture (signe, msg 249/231)
    /// vers `wan_walker` (ou `lan_walker` si meme IP WAN que nous).
    fn on_puncture_request(&self, _src: UdpAddress, pkt: &Packet) -> Result<(), Ipv8Error> {
        let mut r = Reader::new(&pkt.payload);
        let (lan_walker, wan_walker, identifier, new_style) = match pkt.msg_id {
            msg::PUNCTURE_REQUEST => {
                let p = crate::payloads::PunctureRequestPayload::unpack(&mut r)?;
                (
                    p.lan_walker_address,
                    p.wan_walker_address,
                    p.identifier,
                    false,
                )
            }
            msg::NEW_PUNCTURE_REQUEST => {
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
        let target = if same_ip(&wan_walker, &self.my_estimated_wan()) {
            lan_walker
        } else {
            wan_walker.clone()
        };
        let c = self.weak.upgrade();
        if let Some(c) = c {
            let my_lan = c.my_estimated_lan();
            tokio::spawn(async move {
                let mut w = Writer::new();
                let msg_id = if new_style {
                    let _ = w.ip_address(&my_lan);
                    let _ = w.ip_address(&wan_walker);
                    msg::NEW_PUNCTURE
                } else {
                    let _ = w.ipv4(&my_lan);
                    let _ = w.ipv4(&wan_walker);
                    msg::PUNCTURE
                };
                w.u16(identifier);
                let pkt = Packet::sign(
                    &DISCOVERY_COMMUNITY_ID,
                    msg_id,
                    &c.key,
                    c.claim_global_time(),
                    &w.into_bytes(),
                );
                let _ = c.endpoint.send_to(&target, &pkt).await;
            });
        }
        Ok(())
    }

    /// `create_puncture_request` : paquet **non signe** demandant a un
    /// pair de nous puncturer vers (`lan_walker`, `wan_walker`).
    fn make_puncture_request(
        &self,
        lan_walker: &UdpAddress,
        wan_walker: &UdpAddress,
        identifier: u16,
        new_style: bool,
    ) -> Vec<u8> {
        let mut w = Writer::new();
        let msg_id = if new_style
            || !matches!(lan_walker, UdpAddress::Ipv4(_))
            || !matches!(wan_walker, UdpAddress::Ipv4(_))
        {
            let _ = w.ip_address(lan_walker);
            let _ = w.ip_address(wan_walker);
            msg::NEW_PUNCTURE_REQUEST
        } else {
            let _ = w.ipv4(lan_walker);
            let _ = w.ipv4(wan_walker);
            msg::PUNCTURE_REQUEST
        };
        w.u16(identifier);
        Packet::pack_unsigned(
            &DISCOVERY_COMMUNITY_ID,
            msg_id,
            self.claim_global_time(),
            &w.into_bytes(),
        )
    }

    /// `endpoint.send(create_puncture_request(...))` pyipv8 : envoie
    /// une `puncture-request` **non signee** (msg 250 ancien, 232
    /// nouveau si `new_style` ou adresse non-IPv4) a `addr` pour que le
    /// pair puncture (`lan_walker`, `wan_walker`).
    pub async fn send_puncture_request(
        &self,
        addr: &UdpAddress,
        lan_walker: &UdpAddress,
        wan_walker: &UdpAddress,
        new_style: bool,
    ) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        let pkt = self.make_puncture_request(lan_walker, wan_walker, id, new_style);
        self.endpoint.send_to(addr, &pkt).await?;
        Ok(id)
    }

    /// Envoie un `ping` (msg 3).
    pub async fn send_ping(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        self.send_payload(addr, &Ping { identifier: id }).await?;
        Ok(id)
    }

    /// Envoie une `similarity-request` (msg 1).
    pub async fn send_similarity_request(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        let p = SimilarityRequest {
            identifier: id,
            lan_address: self.my_estimated_lan(),
            wan_address: self.my_estimated_wan(),
            connection_type: ConnectionType::Unknown,
            preference_list: vec![DISCOVERY_COMMUNITY_ID],
        };
        self.send_payload(addr, &p).await?;
        Ok(id)
    }

    /// Envoie une `introduction-request` (msg 246 ancien, 234 nouveau
    /// si `is_new_style` ou adresse non-IPv4).
    pub async fn send_introduction_request(&self, addr: &UdpAddress) -> Result<u16, Ipv8Error> {
        let id = self.next_id();
        let new_style = self.network.is_new_style(addr) || !matches!(addr, UdpAddress::Ipv4(_));
        if new_style {
            let p = NewIntroductionRequest {
                destination_address: addr.clone(),
                source_lan_address: self.my_estimated_lan(),
                source_wan_address: self.my_estimated_wan(),
                identifier: id,
                connection_type: ConnectionType::Unknown,
                supports_new_style: true,
                tunnel: false,
                sync: false,
                advice: true,
                extra_bytes: Vec::new(),
            };
            let mut w = Writer::new();
            p.pack(&mut w)?;
            let pkt = Packet::sign(
                &DISCOVERY_COMMUNITY_ID,
                msg::NEW_INTRODUCTION_REQUEST,
                &self.key,
                self.claim_global_time() % 65536,
                &w.into_bytes(),
            );
            self.endpoint.send_to(addr, &pkt).await?;
        } else {
            let p = IntroductionRequest {
                destination_address: addr.clone(),
                source_lan_address: self.my_estimated_lan(),
                source_wan_address: self.my_estimated_wan(),
                advice: true,
                supports_new_style: true,
                connection_type: ConnectionType::Unknown,
                identifier: id,
                extra_bytes: Vec::new(),
            };
            self.send_payload(addr, &p).await?;
        }
        self.pending_intro
            .lock()
            .unwrap()
            .insert(id, (addr.clone(), Instant::now()));
        Ok(id)
    }

    /// `Community.get_new_introduction` : introduction-request vers un
    /// pair verifie de l'overlay (choisi au hasard), sinon bootstrap.
    /// Avec 5 % de chance, re-bootstrap (`reparation d'un reseau
    /// partitionne`).
    pub async fn get_new_introduction(&self, bootstrap: &[UdpAddress]) -> Result<(), Ipv8Error> {
        // `ThreadRng` n'est pas `Send` : tout le tirage est fait dans
        // des blocs separes, jamais a travers un `.await`.
        let bootstrap = self.merged_bootstrap(bootstrap);
        let available = self.network.peers_for_service(&DISCOVERY_COMMUNITY_ID);
        if !available.is_empty() {
            // Petit hasard de reparation d'un reseau partitionne.
            let rebootstrap = !bootstrap.is_empty() && rand::random::<f64>() < REBOOTSTRAP_CHANCE;
            let target = {
                let mut rng = rand::thread_rng();
                if rebootstrap {
                    bootstrap.choose(&mut rng).cloned()
                } else {
                    available.choose(&mut rng).and_then(|p| p.address.clone())
                }
            };
            if let Some(addr) = target {
                return self.send_introduction_request(&addr).await.map(|_| ());
            }
        }
        // Aucun pair verifie : marche vers un noeud d'amorcage.
        let target = {
            let mut rng = rand::thread_rng();
            bootstrap.choose(&mut rng).cloned()
        };
        if let Some(addr) = target {
            self.send_introduction_request(&addr).await?;
        }
        Ok(())
    }

    /// `RandomChurn.take_step` Python : echantillonne des pairs
    /// verifies, pingue ceux devenus inactifs (`CHURN_INACTIVE`) et
    /// supprime de l'annuaire ceux deja pingues mais toujours sans
    /// reponse apres `CHURN_DROP` (57,5 s). Sans ce mecanisme, un
    /// pair mort resterait indefiniment dans `peers_for_service` et
    /// serait sans cesse re-elu saut de circuit tunnel.
    async fn churn_step(&self) {
        let peers = self.network.all_verified_peers();
        let n = peers.len().min(CHURN_SAMPLE_SIZE);
        if n == 0 {
            return;
        }
        let window: Vec<Peer> = {
            let mut rng = rand::thread_rng();
            peers.choose_multiple(&mut rng, n).cloned().collect()
        };
        for peer in window {
            let Some(addr) = peer.address.clone() else {
                continue;
            };
            let inactivity = peer.last_response_elapsed();
            enum Action {
                None,
                Ping,
                Drop,
            }
            let action = {
                let mut pinged = self.churn_pinged.lock().unwrap();
                if inactivity >= CHURN_DROP && pinged.contains_key(&addr) {
                    // `should_drop` : deja pingue, toujours sans reponse.
                    pinged.remove(&addr);
                    Action::Drop
                } else if inactivity >= CHURN_INACTIVE {
                    // `is_inactive` : nouveau ping si le precedent date
                    // de plus de `CHURN_PING_INTERVAL`.
                    if pinged
                        .get(&addr)
                        .is_some_and(|t| t.elapsed() >= CHURN_PING_INTERVAL)
                    {
                        pinged.remove(&addr);
                    }
                    if !pinged.contains_key(&addr) {
                        pinged.insert(addr.clone(), Instant::now());
                        Action::Ping
                    } else {
                        Action::None
                    }
                } else {
                    Action::None
                }
            };
            match action {
                Action::Ping => {
                    let _ = self.send_ping(&addr).await;
                }
                Action::Drop => {
                    tracing::debug!(
                        addr = ?addr,
                        mid = %hex::encode(peer.mid),
                        "churn : pair inerte supprime de l'annuaire"
                    );
                    self.network.remove_peer_key(&peer.public_key_bin);
                }
                Action::None => {}
            }
        }
    }

    /// `RandomWalk.take_step` Python : **les adresses "walkable"**
    /// (apprises par introduction-response, pas encore verifiees)
    /// d'abord — ~80 % des etapes ; sinon `get_new_introduction`
    /// (pair connu ou bootstrap). Une adresse sans reponse apres
    /// `INTRO_TIMEOUT` est oubliee ; `INTRO_WINDOW` introductions en
    /// vol suspendent la marche.
    pub async fn step(&self, bootstrap: &[UdpAddress]) -> Result<(), Ipv8Error> {
        // `RandomChurn` : strategie independante en tete d'etape (les
        // strategies pyipv8 tournent toutes au meme tick de
        // `walker_interval`).
        self.churn_step().await;

        // Purge des adresses mortes (`node_timeout`) : sans pair
        // verifie a cette adresse, on l'oublie completement.
        let mut expired = Vec::new();
        {
            let mut timeouts = self.intro_timeouts.lock().unwrap();
            timeouts.retain(|addr, t| {
                if t.elapsed() >= INTRO_TIMEOUT {
                    expired.push(addr.clone());
                    false
                } else {
                    true
                }
            });
        }
        for addr in expired {
            if self.network.get_verified_by_address(&addr).is_none() {
                self.network.remove_by_address(&addr);
            }
        }
        // Fenetre de vol : assez d'introductions en attente.
        if self.intro_timeouts.lock().unwrap().len() >= INTRO_WINDOW {
            return Ok(());
        }
        let known = self
            .network
            .get_walkable_addresses(Some(&DISCOVERY_COMMUNITY_ID), false);
        let available: Vec<UdpAddress> = {
            let timeouts = self.intro_timeouts.lock().unwrap();
            known
                .into_iter()
                .filter(|a| !timeouts.contains_key(a))
                .collect()
        };
        // randint(0, 255) >= reset_chance -> marche walkable (~80 %).
        if !available.is_empty() && rand::random::<u8>() >= RESET_CHANCE {
            let target = {
                let mut rng = rand::thread_rng();
                available.choose(&mut rng).cloned()
            };
            if let Some(addr) = target {
                self.intro_timeouts
                    .lock()
                    .unwrap()
                    .insert(addr.clone(), Instant::now());
                self.send_introduction_request(&addr).await?;
            }
            return Ok(());
        }
        self.get_new_introduction(bootstrap).await
    }

    /// Boucle de marche aleatoire (a spawner) — cadence
    /// `walker_interval` de la config (`ipv8.walker_interval`, 0,5 s
    /// comme pyipv8), pas un constant local.
    pub async fn run(self: &Arc<Self>, bootstrap: Vec<UdpAddress>, interval: Duration) {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            if let Err(e) = self.step(&bootstrap).await {
                tracing::debug!(error = %e, "etape de marche discovery echouee");
            }
        }
    }

    /// Nombre de pairs verifies connus.
    /// `introduction-response` decodees (233/245) — observable d'interop.
    pub fn intro_response_count(&self) -> usize {
        self.intro_responses_seen
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `introduction-request` decodees et traitees (246/234).
    pub fn intro_request_count(&self) -> usize {
        self.intro_requests_seen
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `puncture` decodees (249/231).
    pub fn puncture_count(&self) -> usize {
        self.punctures_seen
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn peer_count(&self) -> usize {
        self.network.len()
    }

    /// Annuaire reseau partage (acces pour les autres communities).
    pub fn network(&self) -> &Arc<Network> {
        &self.network
    }
}

/// Champs communs des deux formats d'introduction-request
/// (`destination_address` est remplace par l'adresse source reelle du
/// paquet, qui est toujours plus fiable).
struct IntroFields {
    /// `source_lan_address`.
    source_lan_address: UdpAddress,
    /// `source_wan_address`.
    source_wan_address: UdpAddress,
    /// `identifier`.
    identifier: u16,
}

impl From<&IntroductionRequest> for IntroFields {
    fn from(p: &IntroductionRequest) -> Self {
        Self {
            source_lan_address: p.source_lan_address.clone(),
            source_wan_address: p.source_wan_address.clone(),
            identifier: p.identifier,
        }
    }
}

impl From<&NewIntroductionRequest> for IntroFields {
    fn from(p: &NewIntroductionRequest) -> Self {
        Self {
            source_lan_address: p.source_lan_address.clone(),
            source_wan_address: p.source_wan_address.clone(),
            identifier: p.identifier,
        }
    }
}

/// Champs communs des deux formats d'introduction-response.
struct IntroResponseFields {
    /// `destination_address`.
    destination_address: UdpAddress,
    /// `lan_introduction_address`.
    lan_introduction_address: UdpAddress,
    /// `wan_introduction_address`.
    wan_introduction_address: UdpAddress,
    /// `intro_supports_new_style`.
    intro_supports_new_style: bool,
    /// `identifier`.
    #[allow(dead_code)]
    identifier: u16,
}

impl From<&IntroductionResponse> for IntroResponseFields {
    fn from(p: &IntroductionResponse) -> Self {
        Self {
            destination_address: p.destination_address.clone(),
            lan_introduction_address: p.lan_introduction_address.clone(),
            wan_introduction_address: p.wan_introduction_address.clone(),
            intro_supports_new_style: p.intro_supports_new_style,
            identifier: p.identifier,
        }
    }
}

impl From<&NewIntroductionResponse> for IntroResponseFields {
    fn from(p: &NewIntroductionResponse) -> Self {
        Self {
            destination_address: p.destination_address.clone(),
            lan_introduction_address: p.lan_introduction_address.clone(),
            wan_introduction_address: p.wan_introduction_address.clone(),
            intro_supports_new_style: p.intro_supports_new_style,
            identifier: p.identifier,
        }
    }
}

/// `true` si les deux adresses ont la meme IP (peu importe le port).
pub(crate) fn same_ip(a: &UdpAddress, b: &UdpAddress) -> bool {
    match (a, b) {
        (UdpAddress::Ipv4(x), UdpAddress::Ipv4(y)) => x.ip() == y.ip(),
        (UdpAddress::Ipv6(x), UdpAddress::Ipv6(y)) => x.ip() == y.ip(),
        (UdpAddress::Domain(h1, _), UdpAddress::Domain(h2, _)) => h1 == h2,
        _ => false,
    }
}
