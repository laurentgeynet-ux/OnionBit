//! Hidden services / hidden seeding (port de
//! `messaging/anonymization/hidden_services.py` pyipv8) : swarms,
//! points d'introduction, points de rendez-vous, circuits e2e
//! (`create-e2e`/`created-e2e`/`link-e2e`/`linked-e2e`), `peers-request`
//! /`peers-response`.
//!
//! Les messages 13/14 (et 17/18 hors cellule) circulent en paquets
//! **non signes** de prefixe tunnel (`ezr_pack(sig=False)`), soit a nu
//! sur la socket UDP (apres sortie de tunnel), soit a l'interieur
//! d'une cellule `data` (`tunnel_data`).

use std::net::SocketAddr;
use std::sync::Arc;

use rand::seq::SliceRandom;
use tribler_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use tribler_crypto::ipv8::session::{generate_session_keys, Direction, SessionKeys};
use tribler_ipv8::packet::prefix_of;
use tribler_ipv8::peer::Peer;
use tribler_ipv8::serializer::{Reader, Writer};
use tribler_ipv8::{Ipv8Error, UdpAddress};

use crate::community::{
    generate_diffie_secret, generate_diffie_shared_secret, verify_and_generate_shared_secret,
    TunnelCommunity,
};
use crate::payload::{self as tp, msg, Cellable};
use crate::routing::{
    IntroductionPoint, RendezvousPoint, Swarm, CIRCUIT_TYPE_IP_SEEDER, CIRCUIT_TYPE_RP_DOWNLOADER,
    CIRCUIT_TYPE_RP_SEEDER, PEER_SOURCE_DHT,
};
use crate::TUNNEL_COMMUNITY_ID;

/// `PeersResponse` plafond (`random.sample(intro_points, 7)` Python).
const MAX_PEERS_IN_RESPONSE: usize = 7;
/// Nombre d'essais internes d'attente `READY` (poll 20 ms).
const READY_POLL_MS: u64 = 20;

/// `E2ERequestCache` Python : contexte d'un `create-e2e` emis, en
/// attente du `created-e2e`.
pub(crate) struct E2ERequest {
    /// `info_hash` du swarm.
    pub(crate) info_hash: [u8; 20],
    /// Secret DH ephemere local (pour `verify_and_generate_shared_secret`).
    pub(crate) dh_secret: [u8; 32],
    /// `seeder_pk` vise (verifie `auth` contre sa `crypt_pk`).
    pub(crate) seeder_pk: Vec<u8>,
    /// Point d'introduction contacte.
    pub(crate) intro_point: IntroductionPoint,
}

/// `LinkRequestCache` Python : contexte d'un `link-e2e` emis.
pub(crate) struct LinkRequest {
    /// Circuit RP_DOWNLOADER en attente de liaison.
    pub(crate) circuit_id: u32,
    /// `info_hash` du swarm.
    pub(crate) info_hash: [u8; 20],
    /// Cles e2e a poser sur le circuit au `linked-e2e`.
    pub(crate) hs_session_keys: SessionKeys,
}

/// Paquet tunnel non signe : `prefix + msg_id + corps` (`ezr_pack`,
/// `sig=False`).
fn pack_unsigned(msg_id: u8, body: &[u8]) -> Vec<u8> {
    let prefix = prefix_of(&TUNNEL_COMMUNITY_ID);
    let mut out = Vec::with_capacity(prefix.len() + 1 + body.len());
    out.extend_from_slice(&prefix);
    out.push(msg_id);
    out.extend_from_slice(body);
    out
}

/// Adresse factice `0.0.0.0:0`.
fn unspecified_addr() -> UdpAddress {
    UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap())
}

impl TunnelCommunity {
    /// `join_swarm` : rejoint un swarm cache (seeder si `seeding`).
    pub fn join_swarm(&self, info_hash: [u8; 20], hops: usize, seeding: bool) {
        let seeder_sk = seeding.then(LibNaClSecretKey::generate);
        self.inner
            .lock()
            .unwrap()
            .swarms
            .insert(info_hash, Swarm::new(info_hash, hops, seeder_sk));
    }

    /// `leave_swarm`.
    pub fn leave_swarm(&self, info_hash: &[u8; 20]) {
        self.inner.lock().unwrap().swarms.remove(info_hash);
    }

    /// Abonne un receveur aux notifications de liaison e2e
    /// (`e2e_callbacks` Python) — recoit `(circuit_id, info_hash)`.
    pub fn e2e_ready(&self) -> tokio::sync::broadcast::Receiver<(u32, [u8; 20])> {
        self.e2e_ready_tx.subscribe()
    }

    /// `select_circuit_for_infohash` + `create_circuit_for_infohash` :
    /// sauts du swarm, +1 pour `IP_SEEDER`/`RP_DOWNLOADER`.
    fn swarm_circuit_hops(&self, info_hash: &[u8; 20], ctype: &str) -> Option<usize> {
        let inner = self.inner.lock().unwrap();
        let swarm = inner.swarms.get(info_hash)?;
        let mut hops = swarm.hops;
        if ctype == CIRCUIT_TYPE_IP_SEEDER || ctype == CIRCUIT_TYPE_RP_DOWNLOADER {
            hops += 1;
        }
        Some(hops)
    }

    /// Choisit un premier hop au hasard parmi les pairs du service
    /// tunnel (hors nous-meme).
    fn pick_first_hop(&self) -> Option<Peer> {
        let my_pk = self.key.public_key().to_bin();
        let mut peers: Vec<Peer> = self
            .network
            .peers_for_service(&TUNNEL_COMMUNITY_ID)
            .into_iter()
            .filter(|p| p.public_key_bin != my_pk)
            .collect();
        peers.shuffle(&mut rand::thread_rng());
        peers.into_iter().next()
    }

    /// Attend qu'un circuit soit `READY` (borne `timeout_ms` passes en
    /// parametre par l'appelant).
    pub async fn wait_circuit_ready(
        &self,
        circuit_id: u32,
        timeout_ms: u64,
    ) -> Result<(), Ipv8Error> {
        let mut waited = 0;
        loop {
            let ready = self
                .inner
                .lock()
                .unwrap()
                .circuits
                .get(&circuit_id)
                .map(|c| c.state() == crate::routing::CIRCUIT_STATE_READY);
            match ready {
                Some(true) => return Ok(()),
                Some(false) if waited < timeout_ms => {
                    tokio::time::sleep(std::time::Duration::from_millis(READY_POLL_MS)).await;
                    waited += READY_POLL_MS;
                }
                Some(false) => return Err(Ipv8Error::Malformed("timeout d'attente circuit READY")),
                None => return Err(Ipv8Error::Malformed("circuit disparu")),
            }
        }
    }

    /// `tunnel_data` : paquet tunnel non signe embarque dans une
    /// cellule `data` (`pre`/`post` Python : sur un `Circuit` la
    /// destination devient `dest_address` -> sortie UDP brute ; sur un
    /// `TunnelExitSocket` elle devient `org_address` -> retour vers
    /// l'amont).
    pub(crate) async fn tunnel_data(
        &self,
        circuit_id: u32,
        destination: &UdpAddress,
        packet: &[u8],
    ) -> Result<(), Ipv8Error> {
        enum Via {
            Circuit(UdpAddress),
            Exit(UdpAddress),
        }
        let via = {
            let inner = self.inner.lock().unwrap();
            if let Some(c) = inner.circuits.get(&circuit_id) {
                Via::Circuit(
                    c.first_hop()
                        .and_then(|h| h.address.clone())
                        .ok_or(Ipv8Error::Malformed("circuit sans hop"))?,
                )
            } else if let Some(e) = inner.exit_sockets.get(&circuit_id) {
                Via::Exit(
                    e.hop
                        .address
                        .clone()
                        .ok_or(Ipv8Error::Malformed("exit sans hop"))?,
                )
            } else {
                return Err(Ipv8Error::Malformed("circuit inconnu"));
            }
        };
        let data = tp::Data {
            circuit_id,
            dest_address: match via {
                Via::Circuit(_) => destination.clone(),
                Via::Exit(_) => unspecified_addr(),
            },
            org_address: match via {
                Via::Circuit(_) => unspecified_addr(),
                Via::Exit(_) => destination.clone(),
            },
            data: packet.to_vec(),
        };
        let target = match via {
            Via::Circuit(a) | Via::Exit(a) => a,
        };
        self.send_cell(&target, &data).await
    }

    /// `create_introduction_point` : circuit `IP_SEEDER` vers un pair
    /// (`required_ip` force le noeud d'introduction) puis
    /// `establish-intro`. Retourne le `circuit_id`.
    pub async fn create_introduction_point(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        required_ip: Option<&Peer>,
    ) -> Result<u32, Ipv8Error> {
        let (hops, _seeder_pk) = {
            let inner = self.inner.lock().unwrap();
            let swarm = inner
                .swarms
                .get(&info_hash)
                .ok_or(Ipv8Error::Malformed("swarm inconnu"))?;
            let Some(sk) = &swarm.seeder_sk else {
                return Err(Ipv8Error::Malformed("swarm non seeder"));
            };
            (swarm.hops + 1, sk.public_key().to_bin())
        };
        let first_hop = match required_ip {
            Some(p) => p.clone(),
            None => self
                .pick_first_hop()
                .ok_or(Ipv8Error::Malformed("aucun pair tunnel"))?,
        };
        let required_exit = required_ip.map(|p| p.public_key_bin.clone());
        let cid = self
            .create_circuit_typed(
                hops,
                &first_hop,
                CIRCUIT_TYPE_IP_SEEDER,
                required_exit,
                Some(info_hash),
            )
            .await?;
        Ok(cid)
    }

    /// `send_establish_intro` : envoie le `establish-intro` sur un
    /// circuit pret (appele par l'appelant apres `wait_circuit_ready`)
    /// et retourne le receveur complete par `intro-established`.
    pub async fn send_establish_intro(
        self: &Arc<Self>,
        circuit_id: u32,
        info_hash: [u8; 20],
    ) -> Result<tokio::sync::oneshot::Receiver<()>, Ipv8Error> {
        let seeder_pk = {
            let inner = self.inner.lock().unwrap();
            inner
                .swarms
                .get(&info_hash)
                .and_then(|s| s.seeder_sk.as_ref().map(|k| k.public_key().to_bin()))
                .ok_or(Ipv8Error::Malformed("swarm inconnu ou non seeder"))?
        };
        let identifier = self.next_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .lock()
            .unwrap()
            .ip_requests
            .insert(identifier, tx);
        let p = tp::EstablishIntro {
            circuit_id,
            identifier,
            info_hash,
            public_key: seeder_pk,
        };
        let addr = {
            let inner = self.inner.lock().unwrap();
            inner
                .circuits
                .get(&circuit_id)
                .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                .ok_or(Ipv8Error::Malformed("circuit inconnu"))?
        };
        self.send_cell(&addr, &p).await?;
        Ok(rx)
    }

    /// `on_establish_intro` : on devient le point d'introduction
    /// (`intro_point_for[seeder_pk] = (exit cid, info_hash)`).
    pub(crate) fn on_establish_intro(
        self: &Arc<Self>,
        src: SocketAddr,
        p: tp::EstablishIntro,
        cid: u32,
    ) {
        let exit_cid = {
            let mut inner = self.inner.lock().unwrap();
            if inner.intro_point_for.contains_key(&p.public_key) {
                tracing::debug!("intro point deja connu pour cette cle");
                return;
            }
            if !inner.exit_sockets.contains_key(&cid) {
                tracing::debug!("establish-intro sans exit socket {}", cid);
                return;
            }
            inner
                .intro_point_for
                .insert(p.public_key.clone(), (cid, p.info_hash));
            cid
        };
        let reply = tp::IntroEstablished {
            circuit_id: exit_cid,
            identifier: p.identifier,
        };
        let addr = UdpAddress::from(src);
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.send_cell(&addr, &reply).await;
        });
    }

    /// `on_intro_established` : complete l'attente `IPRequestCache`.
    pub(crate) fn on_intro_established(self: &Arc<Self>, p: tp::IntroEstablished) {
        if let Some(tx) = self.inner.lock().unwrap().ip_requests.remove(&p.identifier) {
            let _ = tx.send(());
        }
    }

    /// `create_rendezvous_point` : circuit `RP_SEEDER` + `establish-
    /// rendezvous` ; retourne le `RendezvousPoint` pret.
    pub async fn create_rendezvous_point(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        timeout_ms: u64,
    ) -> Result<RendezvousPoint, Ipv8Error> {
        let hops = self
            .swarm_circuit_hops(&info_hash, CIRCUIT_TYPE_RP_SEEDER)
            .ok_or(Ipv8Error::Malformed("swarm inconnu"))?;
        let first_hop = self
            .pick_first_hop()
            .ok_or(Ipv8Error::Malformed("aucun pair tunnel"))?;
        let cid = self
            .create_circuit_typed(
                hops,
                &first_hop,
                CIRCUIT_TYPE_RP_SEEDER,
                None,
                Some(info_hash),
            )
            .await?;
        self.wait_circuit_ready(cid, timeout_ms).await?;

        let mut cookie = [0u8; 20];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut cookie);
        let identifier = self.next_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .lock()
            .unwrap()
            .rp_requests
            .insert(identifier, tx);
        let addr = {
            let inner = self.inner.lock().unwrap();
            inner
                .circuits
                .get(&cid)
                .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                .ok_or(Ipv8Error::Malformed("circuit inconnu"))?
        };
        self.send_cell(
            &addr,
            &tp::EstablishRendezvous {
                circuit_id: cid,
                identifier,
                cookie,
            },
        )
        .await?;
        let rp_addr = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx)
            .await
            .map_err(|_| Ipv8Error::Malformed("timeout rendezvous-established"))?
            .map_err(|_| Ipv8Error::Malformed("cache rp abandonne"))?;
        Ok(RendezvousPoint {
            circuit: cid,
            cookie,
            address: Some(rp_addr),
        })
    }

    /// `on_establish_rendezvous` : on devient point de rendez-vous
    /// (`rendezvous_point_for[cookie] = exit cid`).
    pub(crate) fn on_establish_rendezvous(
        self: &Arc<Self>,
        src: SocketAddr,
        p: tp::EstablishRendezvous,
        cid: u32,
    ) {
        {
            let mut inner = self.inner.lock().unwrap();
            if !inner.exit_sockets.contains_key(&cid) {
                return;
            }
            inner.rendezvous_point_for.insert(p.cookie, cid);
        }
        let my_addr = self
            .endpoint
            .local_addr()
            .map(UdpAddress::from)
            .unwrap_or_else(|_| unspecified_addr());
        let reply = tp::RendezvousEstablished {
            circuit_id: cid,
            identifier: p.identifier,
            rendezvous_point: my_addr,
        };
        let addr = UdpAddress::from(src);
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.send_cell(&addr, &reply).await;
        });
    }

    /// `on_rendezvous_established` : complete `RPRequestCache`.
    pub(crate) fn on_rendezvous_established(self: &Arc<Self>, p: tp::RendezvousEstablished) {
        if let Some(tx) = self.inner.lock().unwrap().rp_requests.remove(&p.identifier) {
            let _ = tx.send(p.rendezvous_point);
        }
    }

    /// `send_peers_request` : demande de peers via un point
    /// d'introduction (`Some`) ou via la sortie d'un circuit (`None` —
    /// chemin DHT, non implemente sans `dht_provider`).
    pub async fn send_peers_request(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        target: Option<&IntroductionPoint>,
        timeout_ms: u64,
    ) -> Result<Vec<IntroductionPoint>, Ipv8Error> {
        let hops = {
            let inner = self.inner.lock().unwrap();
            inner.swarms.get(&info_hash).map(|s| s.hops).unwrap_or(1)
        };
        let cid = self
            .ready_circuits_of_hops(hops)
            .first()
            .copied()
            .ok_or(Ipv8Error::Malformed("aucun circuit pour peers-request"))?;
        let identifier = self.next_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .lock()
            .unwrap()
            .peers_requests
            .insert(identifier, tx);
        let p = tp::PeersRequest {
            circuit_id: cid,
            identifier,
            info_hash,
        };
        match target {
            Some(ip) => {
                let mut w = Writer::new();
                p.pack(&mut w)?;
                self.tunnel_data(
                    cid,
                    &ip.address,
                    &pack_unsigned(msg::PEERS_REQUEST, &w.into_bytes()),
                )
                .await?;
            }
            None => {
                let addr = {
                    let inner = self.inner.lock().unwrap();
                    inner
                        .circuits
                        .get(&cid)
                        .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                        .ok_or(Ipv8Error::Malformed("circuit sans hop"))?
                };
                self.send_cell(&addr, &p).await?;
            }
        }
        tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx)
            .await
            .map_err(|_| Ipv8Error::Malformed("timeout peers-response"))?
            .map_err(|_| Ipv8Error::Malformed("cache peers abandonne"))
    }

    /// `on_peers_request` : repond avec les points d'introduction que
    /// l'on heberge (equivalent PEX ; le chemin DHT de sortie n'est
    /// pas encore branche).
    pub(crate) fn on_peers_request(
        self: &Arc<Self>,
        src: SocketAddr,
        p: tp::PeersRequest,
        circuit_id: Option<u32>,
    ) {
        let my_addr = self
            .endpoint
            .local_addr()
            .map(UdpAddress::from)
            .unwrap_or_else(|_| unspecified_addr());
        let peers: Vec<tp::IntroductionInfo> = {
            let inner = self.inner.lock().unwrap();
            inner
                .intro_point_for
                .iter()
                .filter(|(_, (_, ih))| *ih == p.info_hash)
                .take(MAX_PEERS_IN_RESPONSE)
                .map(|(seeder_pk, _)| tp::IntroductionInfo {
                    address: my_addr.clone(),
                    key: self.key.public_key().to_bin(),
                    seeder_pk: seeder_pk.clone(),
                    source: PEER_SOURCE_DHT,
                })
                .collect()
        };
        let reply = tp::PeersResponse {
            circuit_id: p.circuit_id,
            identifier: p.identifier,
            info_hash: p.info_hash,
            peers,
        };
        let this = self.clone();
        let addr = UdpAddress::from(src);
        tokio::spawn(async move {
            match circuit_id {
                // Reponse dans le tunnel (chiffrement BACKWARD par
                // `send_cell` sur le circuit_id de l'exit).
                Some(_) => {
                    let _ = this.send_cell(&addr, &reply).await;
                }
                // Reponse brute non signee (chemin socket).
                None => {
                    let mut w = Writer::new();
                    let _ = reply.pack(&mut w);
                    let _ = this
                        .endpoint
                        .send_to(&addr, &pack_unsigned(msg::PEERS_RESPONSE, &w.into_bytes()))
                        .await;
                }
            }
        });
    }

    /// `on_peers_response` : complete `PeersRequestCache`.
    pub(crate) fn on_peers_response(self: &Arc<Self>, p: tp::PeersResponse) {
        let tx = self
            .inner
            .lock()
            .unwrap()
            .peers_requests
            .remove(&p.identifier);
        if let Some(tx) = tx {
            let ips = p
                .peers
                .into_iter()
                .filter(|i| !i.address.is_unspecified())
                .map(|i| IntroductionPoint {
                    address: i.address,
                    peer_key: i.key,
                    seeder_pk: i.seeder_pk,
                    source: i.source,
                })
                .collect();
            let _ = tx.send(ips);
        }
    }

    /// `create_e2e` : envoie `create-e2e` au point d'introduction via
    /// `tunnel_data` (paquet non signe dans une cellule `data`).
    pub async fn create_e2e(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        intro_point: &IntroductionPoint,
    ) -> Result<(), Ipv8Error> {
        let hops = {
            let inner = self.inner.lock().unwrap();
            inner.swarms.get(&info_hash).map(|s| s.hops)
        }
        .ok_or(Ipv8Error::Malformed("swarm inconnu"))?;
        let cid = self
            .ready_circuits_of_hops(hops)
            .first()
            .copied()
            .ok_or(Ipv8Error::Malformed("aucun circuit pour e2e"))?;
        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        self.inner.lock().unwrap().e2e_requests.insert(
            identifier,
            E2ERequest {
                info_hash,
                dh_secret,
                seeder_pk: intro_point.seeder_pk.clone(),
                intro_point: intro_point.clone(),
            },
        );
        let p = tp::CreateE2E {
            identifier,
            info_hash,
            node_public_key: intro_point.seeder_pk.clone(),
            key: dh_public.to_vec(),
        };
        let mut w = Writer::new();
        p.pack(&mut w)?;
        self.tunnel_data(
            cid,
            &intro_point.address,
            &pack_unsigned(msg::CREATE_E2E, &w.into_bytes()),
        )
        .await
    }

    /// `on_create_e2e` : a nu sur la socket (`circuit_id=None`) →
    /// forward au seeder via son circuit d'intro ; sur cellule → on
    /// est le seeder : cree le RP et repond `created-e2e`.
    pub(crate) fn on_create_e2e(
        self: &Arc<Self>,
        src: SocketAddr,
        p: tp::CreateE2E,
        circuit_id: Option<u32>,
    ) {
        match circuit_id {
            None => {
                let fwd = {
                    let inner = self.inner.lock().unwrap();
                    inner.intro_point_for.get(&p.node_public_key).copied()
                };
                if let Some((relay_cid, _)) = fwd {
                    let mut w = Writer::new();
                    if p.pack(&mut w).is_err() {
                        return;
                    }
                    let packet = pack_unsigned(msg::CREATE_E2E, &w.into_bytes());
                    let this = self.clone();
                    let dest = UdpAddress::from(src);
                    tokio::spawn(async move {
                        let _ = this.tunnel_data(relay_cid, &dest, &packet).await;
                    });
                } else {
                    tracing::debug!("create-e2e pour seeder_pk inconnu");
                }
            }
            Some(cid) => {
                let seeding = {
                    let inner = self.inner.lock().unwrap();
                    inner
                        .swarms
                        .get(&p.info_hash)
                        .map(|s| s.seeder_sk.is_some())
                        .unwrap_or(false)
                };
                if !seeding {
                    tracing::debug!("create-e2e recu sans swarm seeder");
                    return;
                }
                let this = self.clone();
                tokio::spawn(async move {
                    this.create_created_e2e(p, UdpAddress::from(src), cid).await;
                });
            }
        }
    }

    /// `create_created_e2e` (seeder) : cree le RP, calcule les cles e2e
    /// et repond `created-e2e` au downloader (via le circuit d'intro).
    async fn create_created_e2e(
        self: &Arc<Self>,
        p: tp::CreateE2E,
        requester: UdpAddress,
        intro_circuit: u32,
    ) {
        let rp = match self
            .create_rendezvous_point(p.info_hash, DEFAULT_E2E_TIMEOUT_MS)
            .await
        {
            Ok(rp) => rp,
            Err(e) => {
                tracing::debug!(error = %e, "create_rendezvous_point echoue");
                return;
            }
        };
        let (seeder_sk, rp_exit_key) = {
            let inner = self.inner.lock().unwrap();
            let sk = inner
                .swarms
                .get(&p.info_hash)
                .and_then(|s| s.seeder_sk.clone());
            let rp_key = inner
                .circuits
                .get(&rp.circuit)
                .and_then(|c| c.hops.last().map(|h| h.public_key_bin.clone()));
            (sk, rp_key)
        };
        let (Some(seeder_sk), Some(rp_exit_key)) = (seeder_sk, rp_exit_key) else {
            return;
        };
        let ds = match generate_diffie_shared_secret(&p.key, &seeder_sk) {
            Ok(ds) => ds,
            Err(e) => {
                tracing::debug!(error = %e, "DH e2e echoue");
                return;
            }
        };
        let Ok(session_keys) = generate_session_keys(&ds.shared) else {
            return;
        };
        // Les cles e2e sont posees sur le circuit RP_SEEDER.
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(c) = inner.circuits.get_mut(&rp.circuit) {
                c.hs_session_keys = Some(session_keys.clone());
            }
        }
        // Adresse du RP = `rendezvous_point_addr` de la reponse (le
        // `my_estimated_wan` du noeud RP).
        let rp_info = tp::RendezvousInfo {
            address: rp.address.clone().unwrap_or_else(unspecified_addr),
            key: rp_exit_key,
            cookie: rp.cookie,
        };
        let mut w = Writer::new();
        if rp_info.pack(&mut w).is_err() {
            return;
        }
        let Ok(rp_info_enc) = session_keys
            .clone()
            .encrypt_str(&w.into_bytes(), Direction::Forward)
        else {
            return;
        };
        let reply = tp::CreatedE2E {
            identifier: p.identifier,
            key: ds.crypt_pk.to_vec(),
            auth: ds.auth,
            rp_info_enc,
        };
        let mut w = Writer::new();
        if reply.pack(&mut w).is_err() {
            return;
        }
        let _ = self
            .tunnel_data(
                intro_circuit,
                &requester,
                &pack_unsigned(msg::CREATED_E2E, &w.into_bytes()),
            )
            .await;
    }

    /// `on_created_e2e` (downloader) : verifie l'auth, dechiffre
    /// `rp_info`, cree le circuit `RP_DOWNLOADER` vers le RP puis
    /// envoie `link-e2e`.
    pub(crate) async fn on_created_e2e(
        self: &Arc<Self>,
        p: tp::CreatedE2E,
        _circuit_id: Option<u32>,
    ) {
        let req = {
            self.inner
                .lock()
                .unwrap()
                .e2e_requests
                .remove(&p.identifier)
        };
        let Some(req) = req else {
            tracing::debug!("created-e2e inattendu");
            return;
        };
        let Ok(seeder_pk) = LibNaClPublicKey::from_bin(&req.seeder_pk) else {
            return;
        };
        let Ok(shared) =
            verify_and_generate_shared_secret(&req.dh_secret, &p.key, &p.auth, &seeder_pk.crypt_pk)
        else {
            tracing::debug!("auth created-e2e invalide");
            return;
        };
        let Ok(session_keys) = generate_session_keys(&shared) else {
            return;
        };
        let Ok(rp_bin) = session_keys
            .clone()
            .decrypt_str(&p.rp_info_enc, Direction::Forward)
        else {
            return;
        };
        let Ok(rp_info) = tp::RendezvousInfo::unpack(&mut Reader::new(&rp_bin)) else {
            return;
        };
        // Circuit RP_DOWNLOADER vers le RP (required_exit : le dernier
        // hop doit etre le noeud de rendez-vous).
        let hops = match self.swarm_circuit_hops(&req.info_hash, CIRCUIT_TYPE_RP_DOWNLOADER) {
            Some(h) => h,
            None => return,
        };
        let Some(required) = Peer::new(rp_info.key.clone(), Some(rp_info.address.clone())) else {
            return;
        };
        // Le pair RP n'est pas forcement dans l'annuaire : on
        // l'apprend (son adresse vient du `rp_info` signe DH).
        self.network.add_verified(required);
        self.network
            .discover_service(&rp_info.key, TUNNEL_COMMUNITY_ID);
        let Some(first_hop) = self.pick_first_hop() else {
            return;
        };
        let cid = match self
            .create_circuit_typed(
                hops,
                &first_hop,
                CIRCUIT_TYPE_RP_DOWNLOADER,
                Some(rp_info.key.clone()),
                Some(req.info_hash),
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "circuit RP_DOWNLOADER echoue");
                return;
            }
        };
        // Connection swarm.
        self.inner
            .lock()
            .unwrap()
            .swarms
            .get_mut(&req.info_hash)
            .map(|s| s.connections.insert(cid, req.intro_point.clone()));
        if self
            .wait_circuit_ready(cid, DEFAULT_E2E_TIMEOUT_MS)
            .await
            .is_err()
        {
            return;
        }
        let identifier = self.next_id();
        self.inner.lock().unwrap().link_requests.insert(
            identifier,
            LinkRequest {
                circuit_id: cid,
                info_hash: req.info_hash,
                hs_session_keys: session_keys,
            },
        );
        let addr = {
            let inner = self.inner.lock().unwrap();
            match inner
                .circuits
                .get(&cid)
                .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
            {
                Some(a) => a,
                None => return,
            }
        };
        let _ = self
            .send_cell(
                &addr,
                &tp::LinkE2E {
                    circuit_id: cid,
                    identifier,
                    cookie: rp_info.cookie,
                },
            )
            .await;
    }

    /// `on_link_e2e` (point de rendez-vous) : lie les deux sockets de
    /// sortie en relais `rendezvous_relay` et repond `linked-e2e`.
    pub(crate) fn on_link_e2e(self: &Arc<Self>, src: SocketAddr, p: tp::LinkE2E, circuit_id: u32) {
        let linked = {
            let mut inner = self.inner.lock().unwrap();
            let Some(&relay_cid) = inner.rendezvous_point_for.get(&p.cookie) else {
                tracing::debug!("link-e2e : cookie inconnu");
                return;
            };
            let Some(exit_dl) = inner.exit_sockets.get(&circuit_id) else {
                return;
            };
            if exit_dl.enabled {
                tracing::debug!("link-e2e : exit deja active");
                return;
            }
            let Some(exit_rp) = inner.exit_sockets.get(&relay_cid) else {
                return;
            };
            if exit_rp.enabled {
                return;
            }
            // Detache les deux exits et installe les routes de
            // rendez-vous bidirectionnelles (FORWARD + decrypt/
            // encrypt via `relay_cell`).
            let exit_dl = inner.exit_sockets.remove(&circuit_id).unwrap();
            let exit_rp = inner.exit_sockets.remove(&relay_cid).unwrap();
            inner.relays.insert(
                circuit_id,
                crate::routing::RelayRoute {
                    base: crate::routing::RoutingObject::new(relay_cid),
                    hop: crate::routing::Hop {
                        public_key_bin: exit_rp.hop.public_key_bin.clone(),
                        address: exit_rp.hop.address.clone(),
                        session_keys: exit_dl.hop.session_keys.clone(),
                    },
                    direction: Direction::Forward,
                    rendezvous_relay: true,
                    relay_early_count: 0,
                },
            );
            inner.relays.insert(
                relay_cid,
                crate::routing::RelayRoute {
                    base: crate::routing::RoutingObject::new(circuit_id),
                    hop: crate::routing::Hop {
                        public_key_bin: exit_dl.hop.public_key_bin.clone(),
                        address: exit_dl.hop.address.clone(),
                        session_keys: exit_rp.hop.session_keys.clone(),
                    },
                    direction: Direction::Forward,
                    rendezvous_relay: true,
                    relay_early_count: 0,
                },
            );
            true
        };
        if !linked {
            return;
        }
        let reply = tp::LinkedE2E {
            circuit_id,
            identifier: p.identifier,
        };
        let this = self.clone();
        let addr = UdpAddress::from(src);
        tokio::spawn(async move {
            let _ = this.send_cell(&addr, &reply).await;
        });
    }

    /// `on_linked_e2e` (downloader) : pose `e2e` + `hs_session_keys`
    /// et notifie `e2e_ready`.
    pub(crate) fn on_linked_e2e(self: &Arc<Self>, p: tp::LinkedE2E) {
        let req = self
            .inner
            .lock()
            .unwrap()
            .link_requests
            .remove(&p.identifier);
        let Some(req) = req else {
            tracing::debug!("linked-e2e inattendu");
            return;
        };
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(c) = inner.circuits.get_mut(&req.circuit_id) {
                c.e2e = true;
                c.hs_session_keys = Some(req.hs_session_keys);
                c.base.beat_heart();
            }
        }
        let _ = self.e2e_ready_tx.send((req.circuit_id, req.info_hash));
    }
}

/// Timeout interne des sous-etapes e2e (`create_rendezvous_point`,
/// attente `READY` du RP_DOWNLOADER).
const DEFAULT_E2E_TIMEOUT_MS: u64 = 10_000;
