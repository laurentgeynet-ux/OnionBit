//! `ContentDiscoveryCommunity` — port de
//! `tribler/core/content_discovery/community.py` (Tribler 8.4.3).
//!
//! Gossip de sante de torrents (`HealthRequest`/`Health` msgs 3/4),
//! echange de version (101/102) et select distant (201/202 :
//! `RemoteSelectPayload` json -> `SelectResponsePayload` blob opaque,
//! historiquement un archive LZ4 de mdblob).
//!
//! `community_id` = `9aca62f878969c437da9844cba29a134917e1648`
//! (identique au Python). La community ne connait ni base ni moteur :
//! les donnees passent par le trait [`ContentProvider`] implemente
//! par la couche domaine (`tribler-core`).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;

use crate::address::UdpAddress;
use crate::discovery::{same_ip, DiscoveryCommunity};
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet};
use crate::payloads::{
    msg as base_msg, ConnectionType, IntroductionRequest, IntroductionResponse,
    NewIntroductionRequest, NewIntroductionResponse, Payload,
};
use crate::peer::{Network, Peer};
use crate::serializer::{Reader, Writer};
use crate::CommunityId;

/// `community_id` de `ContentDiscoveryCommunity` (inchange).
pub const CONTENT_DISCOVERY_COMMUNITY_ID: CommunityId = [
    0x9a, 0xca, 0x62, 0xf8, 0x78, 0x96, 0x9c, 0x43, 0x7d, 0xa9, 0x84, 0x4c, 0xba, 0x29, 0xa1, 0x34,
    0x91, 0x7e, 0x16, 0x48,
];

/// Identifiants de messages (`msg_id` Python, inchanges).
pub mod msg {
    /// `HealthRequestPayload`.
    pub const HEALTH_REQUEST: u8 = 3;
    /// `HealthPayload`.
    pub const HEALTH: u8 = 4;
    /// `VersionRequest`.
    pub const VERSION_REQUEST: u8 = 101;
    /// `VersionResponse`.
    pub const VERSION_RESPONSE: u8 = 102;
    /// `RemoteSelectPayload`.
    pub const REMOTE_SELECT: u8 = 201;
    /// `SelectResponsePayload`.
    pub const SELECT_RESPONSE: u8 = 202;
}

/// `HEALTH_REQUEST_POPULAR` (Python) : les torrents les plus sains.
pub const HEALTH_REQUEST_POPULAR: u8 = 1;
/// `HEALTH_REQUEST_RANDOM` (Python) : echantillon aleatoire.
pub const HEALTH_REQUEST_RANDOM: u8 = 2;

/// Intervalle de gossip de sante (`random_torrent_interval` Python,
/// defaut 5 s — resserre pour les tests via le parametre).
const DEFAULT_GOSSIP_INTERVAL: Duration = Duration::from_secs(5);

/// Nombre de pairs auxquels une requete de sante est envoyee par tick
/// (`random.sample(peers, min(len(peers), 5))` Python).
const GOSSIP_REQUEST_FANOUT: usize = 5;

/// Budget maximum d'une liste `HealthPayload` en octets estimes
/// (`<=1200` cote Python — `size += len(tracker) + 38`).
const HEALTH_PAYLOAD_BUDGET: usize = 1200;

/// TTL d'une requete `remote_select` en attente de reponse.
const SELECT_REQUEST_TTL: Duration = Duration::from_secs(30);

/// Cible de pairs de la marche aleatoire (`RandomWalk` du launcher
/// pyipv8 : `target_peers = 20` pour les overlays Tribler).
const WALK_TARGET_PEERS: usize = 20;

/// `SelectRequest.packets_limit` (cache.py) : nombre maximal de
/// paquets `SelectResponse` acceptes pour une requete (anti-spam).
const SELECT_RESPONSE_PACKETS_LIMIT: u8 = 10;

/// `processing_callback` Python : appelee avec les `to_simple_dict()`
/// des objets NOUVEAUX (`ObjState.NEW_OBJECT`) de chaque paquet de
/// reponse — relayee en `remote_query_results` par l'appelant.
pub type SelectCallback = Arc<dyn Fn(Vec<serde_json::Value>) + Send + Sync>;

/// `SelectRequest` (cache.py) : contexte d'un `remote_select` sortant.
struct PendingSelect {
    /// Pair interroge (adresse) — la reponse est acceptee uniquement
    /// depuis cette source (role de `hexlify(peer.mid)` dans le
    /// `RequestCache` Python ; l'id est deja unique par processus).
    peer_addr: UdpAddress,
    /// Instant d'emission (TTL `SELECT_REQUEST_TTL`).
    sent_at: Instant,
    /// `packets_limit` Python.
    packets_limit: u8,
    /// `peer_responded` Python : au moins un paquet recu — a
    /// l'expiration, un pair muet est retire du reseau
    /// (`_on_query_timeout` -> `network.remove_peer`).
    peer_responded: bool,
    /// `processing_callback` (`None` pour les selects internes de
    /// resolution de santes — pas de notification GUI).
    callback: Option<SelectCallback>,
}

/// `HealthInfo` filaire (`HealthFormat` : `20s, I, I, Q, varlenHutf8`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthInfo {
    /// Info-hash v1 (20 octets).
    pub infohash: [u8; 20],
    /// Seeders observes.
    pub seeders: u32,
    /// Leechers observes.
    pub leechers: u32,
    /// Horodatage du controle (secondes Unix).
    pub last_check: u64,
    /// URL du tracker source ("" = DHT/gossip).
    pub tracker: String,
}

impl HealthInfo {
    /// Poids estime du format dans `HealthPayload` (`len(tracker)+38`
    /// — la borne Python qui evite la fragmentation UDP).
    fn wire_weight(&self) -> usize {
        self.tracker.len() + 38
    }

    fn pack(&self, w: &mut Writer) {
        w.bytes(&self.infohash);
        w.u32(self.seeders);
        w.u32(self.leechers);
        w.u64(self.last_check);
        w.varlen_h(self.tracker.as_bytes());
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let ih = r.take(20)?;
        let mut infohash = [0u8; 20];
        infohash.copy_from_slice(ih);
        let seeders = r.u32()?;
        let leechers = r.u32()?;
        let last_check = r.u64()?;
        let tracker = String::from_utf8_lossy(r.varlen_h()?).into_owned();
        Ok(Self {
            infohash,
            seeders,
            leechers,
            last_check,
            tracker,
        })
    }
}

/// `HealthRequestPayload` (msg 3) : `B request_type`.
#[derive(Debug, Clone)]
pub struct HealthRequest {
    /// `HEALTH_REQUEST_POPULAR` ou `HEALTH_REQUEST_RANDOM`.
    pub request_type: u8,
}

/// `HealthPayload` (msg 4) : `B response_type, [HealthFormat], raw`.
#[derive(Debug, Clone)]
pub struct HealthPayload {
    /// Miroir du `request_type`.
    pub response_type: u8,
    /// Santes transportees (bornees a `HEALTH_PAYLOAD_BUDGET`).
    pub torrents: Vec<HealthInfo>,
}

impl HealthPayload {
    /// `HealthPayload.create` : tronque la liste au budget estime.
    pub fn create(response_type: u8, healths: Vec<HealthInfo>) -> Self {
        let mut size = 0usize;
        let torrents = healths
            .into_iter()
            .take_while(|h| {
                size += h.wire_weight();
                size <= HEALTH_PAYLOAD_BUDGET
            })
            .collect();
        Self {
            response_type,
            torrents,
        }
    }
}

/// `VersionRequest` (msg 101) : vide.
#[derive(Debug, Clone)]
pub struct VersionRequest;

/// `VersionResponse` (msg 102) : `varlenI version, varlenI platform`.
#[derive(Debug, Clone)]
pub struct VersionResponse {
    /// Chaine de version Tribler.
    pub version: String,
    /// Description de plateforme.
    pub platform: String,
}

/// `RemoteSelectPayload` (msg 201) : `I id, varlenH json`.
#[derive(Debug, Clone)]
pub struct RemoteSelect {
    /// Identifiant de requete (choisi par l'emetteur).
    pub id: u32,
    /// Parametres de requete serialises en JSON.
    pub json: Vec<u8>,
}

/// `SelectResponsePayload` (msg 202) : `I id, raw blob`.
#[derive(Debug, Clone)]
pub struct SelectResponse {
    /// Miroir de `RemoteSelect::id`.
    pub id: u32,
    /// Blob de reponse opaque (archive compressee cote Python).
    pub blob: Vec<u8>,
}

/// Port domaine → community : fournit et absorbe les donnees de
/// decouverte. Implemente par `tribler-core` (base + torrent checker).
pub trait ContentProvider: Send + Sync {
    /// `get_random_torrents`/`get_popular_torrents` : santes a
    /// publier pour `request_type` (inconnues → `[]`).
    fn healths_for(&self, request_type: u8) -> Vec<HealthInfo>;
    /// `process_torrents_health` : integre les santes recues ;
    /// retourne les infohashes nouveaux a resoudre par
    /// `remote_select`.
    fn process_health(&self, healths: &[HealthInfo]) -> Vec<[u8; 20]>;
    /// `metadata_store.get_entries_threaded` : repond a un select
    /// distant — JSON de parametres -> blob de resultat.
    fn remote_select(&self, json: &[u8]) -> Vec<u8>;
    /// `process_compressed_mdblob` cote requeteur : integre une
    /// reponse select (blob opaque) ; retourne les `to_simple_dict()`
    /// des objets NOUVEAUX (`ObjState.NEW_OBJECT` Python) pour la
    /// notification `remote_query_results`.
    fn process_select_response(&self, blob: &[u8]) -> Vec<serde_json::Value>;
    /// `(version, platform)` locales pour `VersionResponse`.
    fn version_info(&self) -> (String, String);
}

/// Community de decouverte de contenu (gossip sante + select distant).
pub struct ContentDiscoveryCommunity {
    /// Identite locale.
    key: LibNaClSecretKey,
    /// Annuaire reseau partage (pairs ayant annonce la community).
    network: Arc<Network>,
    /// Endpoint UDP partage.
    endpoint: Arc<UdpEndpoint>,
    /// Fournisseur de contenu (domaine).
    provider: Arc<dyn ContentProvider>,
    /// Compteur d'identifiants `remote_select` (`I`, non borne a u16).
    select_ids: AtomicU32,
    /// Horloge de Lamport (paquets signes).
    global_time: AtomicU64,
    /// Requetes select emises en attente (`RequestCache` Python :
    /// id unique -> contexte `SelectRequest`).
    pending_selects: Mutex<std::collections::HashMap<u32, PendingSelect>>,
    /// La `DiscoveryCommunity` de la meme stack — partagee pour les
    /// estimations `my_estimated_lan/wan` (un seul endpoint UDP, une
    /// seule paire d'estimations cote Python `IPv8`).
    discovery: Arc<DiscoveryCommunity>,
}

impl ContentDiscoveryCommunity {
    /// Cree la community et s'enregistre aupres de l'endpoint ;
    /// lance la boucle de gossip periodique.
    pub async fn new(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
        provider: Arc<dyn ContentProvider>,
        gossip_interval: Option<Duration>,
        discovery: Arc<DiscoveryCommunity>,
    ) -> Arc<Self> {
        let community = Arc::new(Self {
            key,
            network,
            endpoint: endpoint.clone(),
            provider,
            select_ids: AtomicU32::new(0),
            global_time: AtomicU64::new(0),
            pending_selects: Mutex::new(std::collections::HashMap::new()),
            discovery,
        });
        let prefix = prefix_of(&CONTENT_DISCOVERY_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(
                prefix,
                Arc::new(move |src, pkt| c.on_packet(src, pkt)),
                crate::packet::WIRE_DEFAULT,
            )
            .await;
        let c = community.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(gossip_interval.unwrap_or(DEFAULT_GOSSIP_INTERVAL));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // `tokio::interval` tire un premier tick **immediat** — on
            // l'absorbe : le gossip est periodique, rien a emettre a
            // froid (et un tir immediat dependant de l'ordonnancement
            // rend le timing non deterministe).
            tick.tick().await;
            loop {
                tick.tick().await;
                c.gossip_tick().await;
            }
        });
        community
    }

    /// `update_global_time` : l'horodatage recu fait avancer l'horloge
    /// (Lamport) — les messages de cette community n'emettent pas de
    /// `dist` (`ez_send` pyipv8) mais en recoivent via les intros.
    fn update_global_time(&self, t: u64) {
        self.global_time.fetch_max(t, Ordering::Relaxed);
    }

    /// `claim_global_time` : incremente et retourne l'horodatage.
    fn claim_global_time(&self) -> u64 {
        self.global_time
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Annuaire reseau partage.
    pub fn network(&self) -> &Arc<Network> {
        &self.network
    }

    /// `overlay.walk_to` (`Community.walk_to` →
    /// `create_introduction_request` sous le prefixe de la community) —
    /// utilise par `/api/ipv8/isolation` "bootstrapnode".
    pub async fn walk_to(&self, addr: &UdpAddress) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        crate::payloads::IntroductionRequest {
            destination_address: addr.clone(),
            source_lan_address: self.discovery.my_estimated_lan(),
            source_wan_address: self.discovery.my_estimated_wan(),
            // `advice=True` Python : on demande une introduction a un
            // pair de la community (pas seulement un accusé).
            advice: true,
            supports_new_style: true,
            connection_type: crate::payloads::ConnectionType::Unknown,
            identifier: rand::random::<u16>(),
            extra_bytes: Vec::new(),
        }
        .pack(&mut w)?;
        // `sign_auto` : les intros portent le layout `dist`.
        let pkt = Packet::sign_auto(
            &CONTENT_DISCOVERY_COMMUNITY_ID,
            crate::payloads::msg::INTRODUCTION_REQUEST,
            &self.key,
            self.claim_global_time() % 65536,
            &w.into_bytes(),
        );
        self.endpoint.send_to(addr, &pkt).await
    }

    /// `OverlaySchema` : instantane REST de la community
    /// (`GET /api/ipv8/overlays`).
    pub fn overlay_info(&self, is_isolated: bool) -> crate::overlays::OverlayInfo {
        use crate::overlays::{
            content_discovery_msg_name, overlay_peer, OverlayInfo, OverlayStrategy,
            DEFAULT_MAX_PEERS,
        };
        OverlayInfo {
            community_id: CONTENT_DISCOVERY_COMMUNITY_ID,
            my_peer_hex: hex::encode(self.key.public_key().to_bin()),
            global_time: self.global_time.load(std::sync::atomic::Ordering::Relaxed),
            peers: self
                .network
                .peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID)
                .iter()
                .map(overlay_peer)
                .collect(),
            overlay_name: "ContentDiscoveryCommunity",
            max_peers: DEFAULT_MAX_PEERS,
            is_isolated,
            // La community ne suit pas `my_estimated_*` (le Python les
            // estime par listener ; on rapporte 0.0.0.0:0).
            my_estimated_wan: UdpAddress::unspecified(),
            my_estimated_lan: UdpAddress::unspecified(),
            // `BaseLauncher.get_walk_strategies` Tribler.
            strategies: vec![OverlayStrategy {
                name: "RandomWalk",
                target_peers: 20,
            }],
            decode: content_discovery_msg_name,
        }
    }

    /// Serialise + signe + envoie un payload.
    async fn send_payload<P: ContentPayload>(
        &self,
        addr: &UdpAddress,
        payload: &P,
    ) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        payload.pack(&mut w)?;
        // `ez_send` pyipv8 : sans `dist` (hors intros/punctures).
        let packet = Packet::sign_no_dist(
            &CONTENT_DISCOVERY_COMMUNITY_ID,
            P::MSG_ID,
            &self.key,
            &w.into_bytes(),
        );
        self.endpoint.send_to(addr, &packet).await
    }

    /// `RandomWalk.take_step` sous le prefixe de la community : une
    /// `introduction-request` vers un pair connu de l'overlay (le plus
    /// souvent — il nous introduit a un pair de son choix), une
    /// adresse "walkable" introduite sous ce service, un pair verifie
    /// quelconque (un noeud Tribler porte toutes ses overlays sur le
    /// meme port UDP) ou, en dernier repli, un noeud de bootstrap.
    ///
    /// Sans cette marche l'overlay reste vide : aucun pair ne nous
    /// connait sous `CONTENT_DISCOVERY_COMMUNITY_ID`, le gossip n'a
    /// personne a qui parler et `PUT /api/search/remote` n'interroge
    /// personne.
    pub async fn step(&self, bootstrap: &[UdpAddress]) -> Result<(), Ipv8Error> {
        let known = self
            .network
            .peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID);
        if known.len() >= WALK_TARGET_PEERS {
            return Ok(());
        }
        let walkable = self
            .network
            .get_walkable_addresses(Some(&CONTENT_DISCOVERY_COMMUNITY_ID), false);
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
        // Premier tick immediat absorbe : la marche est periodique,
        // le bootstrap discovery vient deja d'emettre ses intros.
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = self.step(&bootstrap).await {
                tracing::debug!(error = %e, "etape de marche content-discovery echouee");
            }
        }
    }

    /// `on_puncture_request` generique (`Community` pyipv8) : demande
    /// **non signee** de percer un trou NAT vers `wan_walker` (ou
    /// `lan_walker` si meme IP WAN que nous) — on repond par un
    /// `puncture` sous le prefixe de la community.
    fn on_puncture_request(self: &Arc<Self>, pkt: &Packet) {
        let mut r = Reader::new(&pkt.payload);
        let (lan_walker, wan_walker, identifier, new_style) = match pkt.msg_id {
            base_msg::PUNCTURE_REQUEST => {
                match crate::payloads::PunctureRequestPayload::unpack(&mut r) {
                    Ok(p) => (
                        p.lan_walker_address,
                        p.wan_walker_address,
                        p.identifier,
                        false,
                    ),
                    Err(_) => return,
                }
            }
            base_msg::NEW_PUNCTURE_REQUEST => {
                match crate::payloads::NewPunctureRequestPayload::unpack(&mut r) {
                    Ok(p) => (
                        p.lan_walker_address,
                        p.wan_walker_address,
                        p.identifier,
                        true,
                    ),
                    Err(_) => return,
                }
            }
            _ => return,
        };
        let my_wan = self.discovery.my_estimated_wan();
        let target = if same_ip(&wan_walker, &my_wan) {
            lan_walker
        } else {
            wan_walker.clone()
        };
        let c = self.clone();
        let my_lan = self.discovery.my_estimated_lan();
        tokio::spawn(async move {
            let mut w = Writer::new();
            let msg_id = if new_style {
                let _ = w.ip_address(&my_lan);
                let _ = w.ip_address(&wan_walker);
                base_msg::NEW_PUNCTURE
            } else {
                let _ = w.ipv4(&my_lan);
                let _ = w.ipv4(&wan_walker);
                base_msg::PUNCTURE
            };
            w.u16(identifier);
            let pkt = Packet::sign(
                &CONTENT_DISCOVERY_COMMUNITY_ID,
                msg_id,
                &c.key,
                c.claim_global_time(),
                &w.into_bytes(),
            );
            let _ = c.endpoint.send_to(&target, &pkt).await;
        });
    }

    /// `introduction_request_callback` generique : repond au demandeur
    /// en introduisant un pair connu de la community et notifie le
    /// pair introduit par `puncture-request` (`create_introduction_
    /// response` Python — le trou NAT est perce par le puncture).
    fn on_introduction_request(
        self: &Arc<Self>,
        peer: Option<Peer>,
        src_addr: &UdpAddress,
        identifier: u16,
        req_lan: UdpAddress,
        req_wan: UdpAddress,
        new_style: bool,
    ) {
        let Some(peer) = peer else { return };
        self.network.add_verified(peer.clone());
        self.network
            .discover_service(&peer.public_key_bin, CONTENT_DISCOVERY_COMMUNITY_ID);

        // `get_peer_for_introduction` : un pair de l'overlay, hors
        // demandeur, avec une adresse connue.
        let candidates: Vec<Peer> = self
            .network
            .peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID)
            .into_iter()
            .filter(|q| q.public_key_bin != peer.public_key_bin)
            .filter(|q| q.address.as_ref().is_some_and(|a| !a.is_unspecified()))
            .collect();
        let introduction = candidates.choose(&mut rand::thread_rng()).cloned();
        let (lan_i, wan_i) = match introduction.as_ref().and_then(|p| p.address.clone()) {
            Some(a) => (a.clone(), a),
            None => (UdpAddress::unspecified(), UdpAddress::unspecified()),
        };
        let c = self.clone();
        let dst = src_addr.clone();
        let my_lan = self.discovery.my_estimated_lan();
        let wan = self.discovery.my_estimated_wan();
        let my_wan = if wan.is_unspecified() {
            src_addr.clone()
        } else {
            wan
        };
        let intro_new_style = introduction
            .as_ref()
            .map(|p| p.new_style_intro)
            .unwrap_or(false);
        let dest = req_wan.clone();
        tokio::spawn(async move {
            let mut w = Writer::new();
            let res = if new_style {
                NewIntroductionResponse {
                    destination_address: dest.clone(),
                    source_lan_address: my_lan,
                    source_wan_address: my_wan,
                    lan_introduction_address: lan_i,
                    wan_introduction_address: wan_i,
                    identifier,
                    intro_supports_new_style: intro_new_style,
                    extra_bytes: Vec::new(),
                }
                .pack(&mut w)
            } else {
                IntroductionResponse {
                    destination_address: dest,
                    source_lan_address: my_lan,
                    source_wan_address: my_wan,
                    lan_introduction_address: lan_i,
                    wan_introduction_address: wan_i,
                    connection_type: ConnectionType::Unknown,
                    supports_new_style: true,
                    peer_limit_reached: false,
                    identifier,
                    intro_supports_new_style: intro_new_style,
                    extra_bytes: Vec::new(),
                }
                .pack(&mut w)
            };
            if res.is_ok() {
                let pkt = Packet::sign(
                    &CONTENT_DISCOVERY_COMMUNITY_ID,
                    if new_style {
                        base_msg::NEW_INTRODUCTION_RESPONSE
                    } else {
                        base_msg::INTRODUCTION_RESPONSE
                    },
                    &c.key,
                    c.claim_global_time() % 65536,
                    &w.into_bytes(),
                );
                let _ = c.endpoint.send_to(&dst, &pkt).await;
            }
        });

        // `puncture-request` vers le pair introduit (non signee) pour
        // qu'il perce son NAT vers le demandeur.
        if let Some(intro) = introduction {
            if let Some(intro_addr) = intro.address {
                let c = self.clone();
                tokio::spawn(async move {
                    let mut w = Writer::new();
                    let msg_id = if new_style
                        || !matches!(req_lan, UdpAddress::Ipv4(_))
                        || !matches!(req_wan, UdpAddress::Ipv4(_))
                    {
                        let _ = w.ip_address(&req_lan);
                        let _ = w.ip_address(&req_wan);
                        base_msg::NEW_PUNCTURE_REQUEST
                    } else {
                        let _ = w.ipv4(&req_lan);
                        let _ = w.ipv4(&req_wan);
                        base_msg::PUNCTURE_REQUEST
                    };
                    w.u16(identifier);
                    let pkt = Packet::pack_unsigned(
                        &CONTENT_DISCOVERY_COMMUNITY_ID,
                        msg_id,
                        c.claim_global_time(),
                        &w.into_bytes(),
                    );
                    let _ = c.endpoint.send_to(&intro_addr, &pkt).await;
                });
            }
        }
    }

    /// `introduction_response_callback` generique : le repondant
    /// devient un pair de l'overlay et les adresses introduites
    /// rejoignent les walkables du service content-discovery.
    fn on_introduction_response(
        &self,
        peer: Option<Peer>,
        lan_i: UdpAddress,
        wan_i: UdpAddress,
        new_style: bool,
    ) {
        let Some(peer) = peer else { return };
        self.network.add_verified(peer.clone());
        self.network
            .discover_service(&peer.public_key_bin, CONTENT_DISCOVERY_COMMUNITY_ID);
        for addr in [lan_i, wan_i] {
            if !addr.is_unspecified() {
                self.network.discover_address(
                    &peer,
                    addr,
                    Some(CONTENT_DISCOVERY_COMMUNITY_ID),
                    new_style,
                );
            }
        }
    }

    /// `gossip_random_torrents_health` : envoie nos santes vivantes a
    /// un pair aleatoire et demande celles de `GOSSIP_REQUEST_FANOUT`
    /// pairs.
    async fn gossip_tick(&self) {
        // `get_peers` Python : pairs verifies ayant annonce la community.
        let peers: Vec<UdpAddress> = self
            .network
            .peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID)
            .iter()
            .filter_map(|p| p.address.clone())
            .collect();
        if peers.is_empty() {
            return;
        }
        // `ThreadRng` n'est pas `Send` : tout le tirage est fait avant
        // le premier `.await`.
        let (chosen_one, targets) = {
            let mut rng = rand::thread_rng();
            (
                peers.choose(&mut rng).cloned(),
                peers
                    .choose_multiple(&mut rng, GOSSIP_REQUEST_FANOUT)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        if let Some(p) = chosen_one {
            let healths = self.provider.healths_for(HEALTH_REQUEST_RANDOM);
            // Coherent avec le handler `HEALTH_REQUEST` : rien a
            // annoncer -> ne pas emettre de `HealthPayload` vide.
            if !healths.is_empty() {
                let payload = HealthPayload::create(HEALTH_REQUEST_RANDOM, healths);
                let _ = self.send_payload(&p, &payload).await;
            }
        }
        for p in targets {
            let _ = self
                .send_payload(
                    &p,
                    &HealthRequest {
                        request_type: HEALTH_REQUEST_RANDOM,
                    },
                )
                .await;
        }
        // `_on_query_timeout` Python : a l'expiration, un pair qui
        // n'a jamais repondu est retire du reseau (`remove_peer`).
        let now = Instant::now();
        let expired: Vec<PendingSelect> = {
            let mut pend = self.pending_selects.lock().unwrap();
            let ids: Vec<u32> = pend
                .iter()
                .filter(|(_, r)| now.duration_since(r.sent_at) >= SELECT_REQUEST_TTL)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| pend.remove(id)).collect()
        };
        for req in expired {
            if !req.peer_responded {
                self.network.remove_by_address(&req.peer_addr);
            }
        }
    }

    /// `send_remote_select` : demande `json` de parametres au pair ;
    /// la reponse (msg 202) est absorbee par `process_select_response`.
    pub async fn send_remote_select(
        &self,
        peer: &UdpAddress,
        json: Vec<u8>,
    ) -> Result<u32, Ipv8Error> {
        self.send_select(peer, json, None).await
    }

    /// `send_remote_select` avec `processing_callback` — le callback
    /// recoit les objets nouveaux de chaque paquet de reponse
    /// (`send_search_request` -> `remote_query_results` Python).
    pub async fn send_remote_select_cb(
        &self,
        peer: &UdpAddress,
        json: Vec<u8>,
        callback: SelectCallback,
    ) -> Result<u32, Ipv8Error> {
        self.send_select(peer, json, Some(callback)).await
    }

    async fn send_select(
        &self,
        peer: &UdpAddress,
        json: Vec<u8>,
        callback: Option<SelectCallback>,
    ) -> Result<u32, Ipv8Error> {
        let id = self.select_ids.fetch_add(1, Ordering::Relaxed);
        self.pending_selects.lock().unwrap().insert(
            id,
            PendingSelect {
                peer_addr: peer.clone(),
                sent_at: Instant::now(),
                packets_limit: SELECT_RESPONSE_PACKETS_LIMIT,
                peer_responded: false,
                callback,
            },
        );
        self.send_payload(peer, &RemoteSelect { id, json }).await?;
        Ok(id)
    }

    /// Demande les santes d'un pair (`HealthRequestPayload`).
    pub async fn request_health(
        &self,
        peer: &UdpAddress,
        request_type: u8,
    ) -> Result<(), Ipv8Error> {
        self.send_payload(peer, &HealthRequest { request_type })
            .await
    }

    /// Demande la version d'un pair (`send_version_request` Python
    /// sert au ping de version sur nouvelle sante inconnue).
    pub async fn send_version_request(&self, peer: &UdpAddress) -> Result<(), Ipv8Error> {
        self.send_payload(peer, &VersionRequest).await
    }

    /// Dispatch par `msg_id`.
    fn on_packet(self: &Arc<Self>, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        if !pkt.signed {
            // `puncture-request` est le seul message non signe du
            // protocole de marche (comme `DiscoveryCommunity`).
            self.on_puncture_request(&pkt);
            return Ok(());
        }
        self.update_global_time(pkt.global_time);
        let src_addr = UdpAddress::from(src);
        let mut peer = Peer::new(pkt.public_key_bin.clone(), Some(src_addr.clone()));
        if let Some(p) = &peer {
            self.network.add_verified(p.clone());
            self.network
                .discover_service(&pkt.public_key_bin, CONTENT_DISCOVERY_COMMUNITY_ID);
        }
        let mut r = Reader::new(&pkt.payload);
        match pkt.msg_id {
            base_msg::INTRODUCTION_REQUEST => {
                let p = IntroductionRequest::unpack(&mut r)?;
                self.on_introduction_request(
                    peer,
                    &src_addr,
                    p.identifier,
                    p.source_lan_address,
                    p.source_wan_address,
                    false,
                );
            }
            base_msg::NEW_INTRODUCTION_REQUEST => {
                let p = NewIntroductionRequest::unpack(&mut r)?;
                if let Some(ref mut pr) = peer {
                    pr.new_style_intro = true;
                }
                self.on_introduction_request(
                    peer,
                    &src_addr,
                    p.identifier,
                    p.source_lan_address,
                    p.source_wan_address,
                    true,
                );
            }
            base_msg::INTRODUCTION_RESPONSE => {
                let p = IntroductionResponse::unpack(&mut r)?;
                if let Some(ref mut pr) = peer {
                    pr.new_style_intro = p.supports_new_style;
                }
                self.on_introduction_response(
                    peer,
                    p.lan_introduction_address,
                    p.wan_introduction_address,
                    false,
                );
            }
            base_msg::NEW_INTRODUCTION_RESPONSE => {
                let p = NewIntroductionResponse::unpack(&mut r)?;
                if let Some(ref mut pr) = peer {
                    pr.new_style_intro = true;
                }
                self.on_introduction_response(
                    peer,
                    p.lan_introduction_address,
                    p.wan_introduction_address,
                    true,
                );
            }
            base_msg::PUNCTURE | base_msg::NEW_PUNCTURE => {
                // `on_puncture` Python : no-op — le trou NAT est
                // ouvert par la reception meme du paquet.
            }
            msg::HEALTH_REQUEST => {
                let req = HealthRequest::unpack(&mut r)?;
                let c = self.clone();
                tokio::spawn(async move {
                    let healths = c.provider.healths_for(req.request_type);
                    if !healths.is_empty() {
                        let payload = HealthPayload::create(req.request_type, healths);
                        let _ = c.send_payload(&src_addr, &payload).await;
                    }
                });
            }
            msg::HEALTH => {
                let p = HealthPayload::unpack(&mut r)?;
                let to_resolve = self.provider.process_health(&p.torrents);
                let c = self.clone();
                tokio::spawn(async move {
                    for ih in to_resolve {
                        // `send_remote_select(infohash=…, last=1)` Python.
                        let json = serde_json::json!({
                            "infohash": hex::encode(ih),
                            "last": 1
                        });
                        let _ = c
                            .send_remote_select(&src_addr, json.to_string().into_bytes())
                            .await;
                    }
                });
            }
            msg::VERSION_REQUEST => {
                let (version, platform) = self.provider.version_info();
                let c = self.clone();
                tokio::spawn(async move {
                    let _ = c
                        .send_payload(&src_addr, &VersionResponse { version, platform })
                        .await;
                });
            }
            msg::VERSION_RESPONSE => {
                let _ = VersionResponse::unpack(&mut r)?;
            }
            msg::REMOTE_SELECT => {
                let p = RemoteSelect::unpack(&mut r)?;
                let blob = self.provider.remote_select(&p.json);
                let c = self.clone();
                tokio::spawn(async move {
                    let _ = c
                        .send_payload(&src_addr, &SelectResponse { id: p.id, blob })
                        .await;
                });
            }
            msg::SELECT_RESPONSE => {
                let p = SelectResponse::unpack(&mut r)?;
                // `request_cache.get(mid, id)` Python : l'id est
                // unique par processus ; on verifie la source.
                let callback = {
                    let mut pend = self.pending_selects.lock().unwrap();
                    match pend.get_mut(&p.id) {
                        Some(req) if req.peer_addr == src_addr => {
                            req.peer_responded = true;
                            req.packets_limit -= 1;
                            let cb = req.callback.clone();
                            if req.packets_limit == 0 {
                                pend.remove(&p.id);
                            }
                            Some(cb)
                        }
                        _ => None,
                    }
                };
                let Some(callback) = callback else {
                    return Ok(());
                };
                let results = self.provider.process_select_response(&p.blob);
                if let Some(cb) = callback {
                    cb(results);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Trait interne de serialisation des payloads de cette community.
pub trait ContentPayload: Sized {
    /// `msg_id` dans la community.
    const MSG_ID: u8;
    /// Serialise dans `w`.
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error>;
    /// Deserialise depuis `r`.
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error>;
}

impl ContentPayload for HealthRequest {
    const MSG_ID: u8 = msg::HEALTH_REQUEST;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u8(self.request_type);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            request_type: r.u8()?,
        })
    }
}

impl ContentPayload for HealthPayload {
    const MSG_ID: u8 = msg::HEALTH;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u8(self.response_type);
        w.u8(self.torrents.len() as u8);
        for h in &self.torrents {
            h.pack(w);
        }
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let response_type = r.u8()?;
        let count = r.u8()? as usize;
        let mut torrents = Vec::with_capacity(count);
        for _ in 0..count {
            torrents.push(HealthInfo::unpack(r)?);
        }
        // `raw extra_bytes` : ignore (compatibilite ascendante).
        Ok(Self {
            response_type,
            torrents,
        })
    }
}

impl ContentPayload for VersionRequest {
    const MSG_ID: u8 = msg::VERSION_REQUEST;
    fn pack(&self, _w: &mut Writer) -> Result<(), Ipv8Error> {
        Ok(())
    }
    fn unpack(_r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self)
    }
}

impl ContentPayload for VersionResponse {
    const MSG_ID: u8 = msg::VERSION_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.bytes(&(self.version.len() as u32).to_be_bytes());
        w.bytes(self.version.as_bytes());
        w.bytes(&(self.platform.len() as u32).to_be_bytes());
        w.bytes(self.platform.as_bytes());
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let vlen = r.u32()? as usize;
        let version = String::from_utf8_lossy(r.take(vlen)?).into_owned();
        let plen = r.u32()? as usize;
        let platform = String::from_utf8_lossy(r.take(plen)?).into_owned();
        Ok(Self { version, platform })
    }
}

impl ContentPayload for RemoteSelect {
    const MSG_ID: u8 = msg::REMOTE_SELECT;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.id);
        w.varlen_h(&self.json);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            id: r.u32()?,
            json: r.varlen_h()?.to_vec(),
        })
    }
}

impl ContentPayload for SelectResponse {
    const MSG_ID: u8 = msg::SELECT_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.id);
        w.raw(&self.blob);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            id: r.u32()?,
            blob: r.raw().to_vec(),
        })
    }
}
