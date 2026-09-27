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
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet};
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
    /// reponse select (blob opaque).
    fn process_select_response(&self, blob: &[u8]);
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
    /// Requetes select emises en attente (id -> instant).
    pending_selects: Mutex<std::collections::HashMap<u32, Instant>>,
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
    ) -> Arc<Self> {
        let community = Arc::new(Self {
            key,
            network,
            endpoint: endpoint.clone(),
            provider,
            select_ids: AtomicU32::new(0),
            global_time: AtomicU64::new(0),
            pending_selects: Mutex::new(std::collections::HashMap::new()),
        });
        let prefix = prefix_of(&CONTENT_DISCOVERY_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(prefix, Arc::new(move |src, pkt| c.on_packet(src, pkt)))
            .await;
        let c = community.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(gossip_interval.unwrap_or(DEFAULT_GOSSIP_INTERVAL));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                c.gossip_tick().await;
            }
        });
        community
    }

    /// `claim_global_time`.
    fn claim_global_time(&self) -> u64 {
        self.global_time.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn update_global_time(&self, t: u64) {
        self.global_time.fetch_max(t, Ordering::Relaxed);
    }

    /// Serialise + signe + envoie un payload.
    async fn send_payload<P: ContentPayload>(
        &self,
        addr: &UdpAddress,
        payload: &P,
    ) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        payload.pack(&mut w)?;
        let packet = Packet::sign(
            &CONTENT_DISCOVERY_COMMUNITY_ID,
            P::MSG_ID,
            &self.key,
            self.claim_global_time() % 65536,
            &w.into_bytes(),
        );
        self.endpoint.send_to(addr, &packet).await
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
            let payload = HealthPayload::create(
                HEALTH_REQUEST_RANDOM,
                self.provider.healths_for(HEALTH_REQUEST_RANDOM),
            );
            let _ = self.send_payload(&p, &payload).await;
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
        // Purge des requetes select expirees.
        let now = Instant::now();
        self.pending_selects
            .lock()
            .unwrap()
            .retain(|_, t| now.duration_since(*t) < SELECT_REQUEST_TTL);
    }

    /// `send_remote_select` : demande `json` de parametres au pair ;
    /// la reponse (msg 202) est absorbee par `process_select_response`.
    pub async fn send_remote_select(
        &self,
        peer: &UdpAddress,
        json: Vec<u8>,
    ) -> Result<u32, Ipv8Error> {
        let id = self.select_ids.fetch_add(1, Ordering::Relaxed);
        self.pending_selects
            .lock()
            .unwrap()
            .insert(id, Instant::now());
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
            return Ok(());
        }
        self.update_global_time(pkt.global_time);
        let src_addr = UdpAddress::from(src);
        if let Some(p) = Peer::new(pkt.public_key_bin.clone(), Some(src_addr.clone())) {
            self.network.add_verified(p);
            self.network
                .discover_service(&pkt.public_key_bin, CONTENT_DISCOVERY_COMMUNITY_ID);
        }
        let mut r = Reader::new(&pkt.payload);
        match pkt.msg_id {
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
                if self.pending_selects.lock().unwrap().remove(&p.id).is_some() {
                    self.provider.process_select_response(&p.blob);
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
