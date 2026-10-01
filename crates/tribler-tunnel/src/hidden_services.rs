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

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

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
    IntroductionPoint, PendingE2e, RendezvousPoint, Swarm, CIRCUIT_STATE_READY,
    CIRCUIT_TYPE_IP_SEEDER, CIRCUIT_TYPE_RP_DOWNLOADER, CIRCUIT_TYPE_RP_SEEDER, PEER_SOURCE_DHT,
    PEER_SOURCE_PEX,
};

/// `PeersRequestCache.timeout_delay` pyipv8 (`RequestCache` : 10 s).
const PEERS_REQUEST_TIMEOUT_MS: u64 = 10_000;
/// `max_requests` de `estimate_swarm_size` pyipv8.
const SWARM_SIZE_MAX_REQUESTS: usize = 10;

/// `PeersResponse` plafond (`random.sample(intro_points, 7)` Python).
const MAX_PEERS_IN_RESPONSE: usize = 7;
/// Plafond du cache `created-e2e` par swarm (dedup des retries seeder).
const MAX_SEEN_E2E: usize = 64;
/// Nombre d'essais internes d'attente `READY` (poll 20 ms).
const READY_POLL_MS: u64 = 20;

/// `TriblerTunnelCommunity.get_lookup_info_hash`
/// (`core/tunnel/community.py`) : identite publique du swarm cache —
/// `SHA-1(b"tribler anonymous download" + hexlify(info_hash))`. Le
/// swarm ne doit JAMAIS etre indexe par l'infohash reel (fuite du
/// contenu telecharge sur les points d'introduction).
pub fn lookup_info_hash(info_hash: &[u8; 20]) -> [u8; 20] {
    let mut data = b"tribler anonymous download".to_vec();
    data.extend_from_slice(hex::encode(info_hash).as_bytes());
    tribler_crypto::hash::sha1(&data)
}

/// `DHTIntroPointPayload` (`["ip_address","I","varlenH","varlenH"]`)
/// → `IntroductionPoint` : les cles stockees sans prefixe retrouvent
/// `LibNaCLPK:`, la source est `PEER_SOURCE_DHT`. `None` si le blob
/// n'est pas decodable (`PackError` Python → valeur ignoree).
pub fn unpack_dht_intro_point(data: &[u8]) -> Option<IntroductionPoint> {
    let mut r = Reader::new(data);
    let address = r.ip_address().ok()?;
    let last_seen = r.u32().ok()?;
    let intro_pk = r.varlen_h().ok()?;
    let seeder_pk = r.varlen_h().ok()?;
    Some(IntroductionPoint {
        address,
        peer_key: [b"LibNaCLPK:".as_slice(), intro_pk].concat(),
        seeder_pk: [b"LibNaCLPK:".as_slice(), seeder_pk].concat(),
        source: PEER_SOURCE_DHT,
        last_seen_secs: last_seen as u64,
    })
}

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
    /// Paquet `create-e2e` complet (prefixe + msg + corps) — re-emis
    /// tel quel par une retentative idempotente.
    pub(crate) packet: Vec<u8>,
}

/// Purge `pending_e2e` si la construction `RP_DOWNLOADER` echoue —
/// desarme (`disarm`) quand l'etape `Link` prend le relais.
struct PendingGuard<'a> {
    community: &'a TunnelCommunity,
    info_hash: [u8; 20],
    intro_point: IntroductionPoint,
    armed: bool,
}

impl<'a> PendingGuard<'a> {
    fn new(community: &'a TunnelCommunity, req: &E2ERequest) -> Self {
        Self {
            community,
            info_hash: req.info_hash,
            intro_point: req.intro_point.clone(),
            armed: true,
        }
    }

    /// La liaison est entreine : l'etape `Link` detient desormais le
    /// pending, le guard ne doit plus rien purger.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Some(s) = self
                .community
                .inner
                .lock()
                .unwrap()
                .swarms
                .get_mut(&self.info_hash)
            {
                s.pending_e2e.remove(&self.intro_point);
            }
        }
    }
}

/// `LinkRequestCache` Python : contexte d'un `link-e2e` emis.
pub(crate) struct LinkRequest {
    /// Circuit RP_DOWNLOADER en attente de liaison.
    pub(crate) circuit_id: u32,
    /// `info_hash` du swarm.
    pub(crate) info_hash: [u8; 20],
    /// Cles e2e a poser sur le circuit au `linked-e2e`.
    pub(crate) hs_session_keys: SessionKeys,
    /// `cookie` de rendez-vous — permet la re-emission du `link-e2e`
    /// lors d'une retentative idempotente.
    pub(crate) cookie: [u8; 20],
}

/// Paquet tunnel non signe : `prefix + msg_id + corps` (`ezr_pack`,
/// `sig=False`).
fn pack_unsigned(cid: &tribler_ipv8::CommunityId, msg_id: u8, body: &[u8]) -> Vec<u8> {
    let prefix = prefix_of(cid);
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
    /// tunnel (hors nous-meme et hors `exclude`).
    ///
    /// `exclude` doit porter la cle du `required_exit` quand le
    /// circuit vise un dernier saut impose (ex. `RP_DOWNLOADER`) :
    /// sans cette exclusion, un tirage malheureux peut choisir CE
    /// MEME pair comme premier saut, puis `on_extended` l'`EXTEND`
    /// vers lui-meme pour satisfaire `required_exit` — un circuit a 2
    /// "sauts" distincts vers le meme pair physique, dont le
    /// etablissement crypto echoue de facon intermittente.
    fn pick_first_hop(&self, exclude: Option<&[u8]>) -> Option<Peer> {
        let my_pk = self.key.public_key().to_bin();
        let mut peers: Vec<Peer> = self
            .network
            .peers_for_service(&self.community_id)
            .into_iter()
            .filter(|p| p.public_key_bin != my_pk && Some(p.public_key_bin.as_slice()) != exclude)
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
        self.send_cell(&target, &data).await.map(|_| ())
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
        // `required_ip` Python devient `required_exit` : le DERNIER saut
        // impose (le point d'introduction lui-meme). Le premier hop reste
        // un relais ordinaire (`possible_first_hops` de `create_circuit`,
        // `required_exit` exclu) — l'imposer aussi comme premier saut
        // produirait un circuit S->A->A que l'etablissement peut refuser.
        // Pour un circuit a 1 saut, `create_circuit_typed` substitue deja
        // `required_exit` comme premier hop (egalite Python
        // `possible_first_hops = [required_exit]`).
        let required_exit = required_ip.map(|p| p.public_key_bin.clone());
        let first_hop = match required_ip {
            // 1 saut : `create_circuit_typed` substitue `required_exit`
            // comme premier hop — le choix importe peu, on evite juste
            // d'echouer sur `pick_first_hop` quand le pair epingle est le
            // seul pair tunnel connu.
            Some(p) if hops == 1 => p.clone(),
            _ => self
                .pick_first_hop(required_exit.as_deref())
                .ok_or(Ipv8Error::Malformed("aucun pair tunnel"))?,
        };
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
            // "Established introduction point for %s" (info).
            tracing::info!(
                circuit_id = cid,
                info_hash = hex::encode(p.info_hash),
                "point d'introduction etabli (nous sommes l'intro point)"
            );
            // `on_establish_intro` Python : cree le store PEX de ce
            // swarm si besoin puis `start_announce(seeder_pk)` — on
            // s'annonce point d'introduction.
            inner
                .pex
                .entry(p.info_hash)
                .or_default()
                .start_announce(p.public_key.clone());
            cid
        };
        // `dht_announce` Python : le point d'introduction publie le
        // `DHTIntroPointPayload` sous la cle du swarm dans la DHT
        // IPv8 — c'est ce que `find_values` du downloader interroge.
        // Sans elle le swarm reste invisible hors PEX.
        self.dht_announce(p.info_hash, p.public_key);
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
        let announce = {
            let mut inner = self.inner.lock().unwrap();
            match inner.ip_requests.remove(&p.identifier) {
                Some(tx) => {
                    // "Established introduction tunnel %s" (info).
                    tracing::info!(circuit_id = p.circuit_id, "intro-established recu");
                    let _ = tx.send(());
                    // Annonce DHT redondante cote seeder : pyipv8
                    // s'en remet entierement au point d'introduction
                    // (`on_establish_intro` → `dht_announce`) — si ce
                    // noeud ne publie pas (provider absent, store en
                    // echec), le swarm reste invisible a jamais et la
                    // dedup `intro_point_for` interdit tout second
                    // essai vers ce noeud. On re-publie le meme
                    // `DHTIntroPointPayload` (adresse+cle du point,
                    // notre seeder_pk) : contenu identique, source
                    // differente — sans effet quand le point a deja
                    // annonce.
                    inner.circuits.get(&p.circuit_id).map(|c| {
                        (
                            c.info_hash,
                            c.hops
                                .last()
                                .map(|h| (h.address.clone(), h.public_key_bin.clone())),
                        )
                    })
                }
                None => None,
            }
        };
        if let Some((Some(info_hash), Some((Some(addr), intro_pk)))) = announce {
            self.announce_intro_circuit(info_hash, &addr, &intro_pk);
        }
    }

    /// Publie le `DHTIntroPointPayload` d'un circuit `IP_SEEDER` pret
    /// depuis le cote seeder (annonce redondante — cf.
    /// `on_intro_established`). `addr`/`intro_pk` decrivent le saut
    /// final (le point d'introduction).
    fn announce_intro_circuit(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        addr: &UdpAddress,
        intro_pk: &[u8],
    ) {
        let seeder_pk = {
            let inner = self.inner.lock().unwrap();
            inner
                .swarms
                .get(&info_hash)
                .and_then(|s| s.seeder_sk.as_ref().map(|k| k.public_key().to_bin()))
        };
        let Some(seeder_pk) = seeder_pk else { return };
        // Point epingle (`intro_point_peer`) : publier l'adresse du
        // pin plutot que le WAN estime du dernier saut — sur un banc
        // local le WAN annonce (NAT, hairpin impossible) serait
        // injoignable pour le downloader.
        let addr = match &self.settings.intro_point_peer {
            Some(pin) => match self.network.get_verified_by_address(pin) {
                Some(peer) if peer.public_key_bin == intro_pk => pin.clone(),
                _ => addr.clone(),
            },
            None => addr.clone(),
        };
        self.dht_store_intro_point(info_hash, &addr, intro_pk, &seeder_pk);
    }

    /// Re-annonce periodique des points d'introduction d'un swarm
    /// `SEEDING` (robustesse : une annonce perdue/diluee rend le swarm
    /// invisible — pyipv8 s'en remet au seul point d'introduction, qui
    /// ne re-publie jamais). Bornee par `intro_reannounce_interval` ;
    /// chaque circuit `IP_SEEDER` pret republie son `DHTIntroPointPayload`.
    pub async fn reannounce_intro_points(self: &Arc<Self>, info_hash: [u8; 20]) {
        let circuits = {
            let mut inner = self.inner.lock().unwrap();
            let last = inner.ip_announced_at.get(&info_hash).copied();
            if last.is_some_and(|t| t.elapsed() < self.settings.intro_reannounce_interval) {
                return;
            }
            inner.ip_announced_at.insert(info_hash, Instant::now());
            inner
                .circuits
                .values()
                .filter(|c| {
                    c.ctype == CIRCUIT_TYPE_IP_SEEDER
                        && c.info_hash == Some(info_hash)
                        && c.state() == CIRCUIT_STATE_READY
                })
                .filter_map(|c| {
                    c.hops
                        .last()
                        .and_then(|h| h.address.clone().map(|a| (a, h.public_key_bin.clone())))
                })
                .collect::<Vec<_>>()
        };
        for (addr, intro_pk) in circuits {
            self.announce_intro_circuit(info_hash, &addr, &intro_pk);
        }
    }

    /// `public_key_bin[10:]` Python : retire le prefixe `LibNaCLPK:`
    /// des cles ecrites dans `DHTIntroPointPayload` (limite de taille
    /// des valeurs DHT — les cles ne portent pas leur prefixe wire).
    fn strip_pk_prefix(pk: &[u8]) -> &[u8] {
        pk.strip_prefix(b"LibNaCLPK:".as_slice()).unwrap_or(pk)
    }

    /// `dht_announce` (`hidden_services.py`) : appele par le noeud qui
    /// accepte un `establish-intro` — publie le `DHTIntroPointPayload`
    /// `(notre_adresse_wan, last_seen, intro_pk, seeder_pk)` sous la
    /// cle `info_hash` (deja `lookup_info_hash`) de la DHT IPv8.
    /// No-op sans `dht_provider` configure.
    fn dht_announce(self: &Arc<Self>, info_hash: [u8; 20], seeder_pk: Vec<u8>) {
        // `IntroductionPoint(Peer(my_peer.key, my_estimated_wan)…)` :
        // l'adresse annoncee est notre WAN estime — repli sur
        // l'adresse d'ecoute locale si l'estimation est encore absente
        // (sinon l'annonce serait inexploitable).
        let wan = self.my_wan();
        let addr = if !wan.is_unspecified() {
            wan
        } else {
            self.endpoint
                .local_addr()
                .map(UdpAddress::from)
                .unwrap_or_else(|_| unspecified_addr())
        };
        let pk = self.key.public_key().to_bin();
        self.dht_store_intro_point(info_hash, &addr, &pk, &seeder_pk);
    }

    /// Publication effective du `DHTIntroPointPayload`
    /// `(intro_addr, last_seen, intro_pk, seeder_pk)` sous la cle
    /// `info_hash` de la DHT IPv8 (`dht_announce` →
    /// `DHTCommunityProvider.announce` → `store_value`).
    /// No-op sans `dht_provider` configure.
    fn dht_store_intro_point(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        intro_addr: &UdpAddress,
        intro_pk: &[u8],
        seeder_pk: &[u8],
    ) {
        let Some(dht) = self.dht_provider() else {
            return;
        };
        let mut w = Writer::new();
        if w.ip_address(intro_addr).is_err() {
            return;
        }
        w.u32(crate::pex::epoch_secs() as u32);
        w.varlen_h(Self::strip_pk_prefix(intro_pk));
        w.varlen_h(Self::strip_pk_prefix(seeder_pk));
        let ih_hex = hex::encode(info_hash);
        tokio::spawn(async move {
            // "Announced %s to the DHTCommunity" (info).
            match dht.store_value(&info_hash, &w.into_bytes(), false).await {
                Ok(_) => tracing::info!(
                    info_hash = ih_hex,
                    "point d'introduction annonce sur la DHT"
                ),
                Err(e) => tracing::debug!(info_hash = ih_hex, error = %e, "dht_announce echoue"),
            }
        });
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
            .pick_first_hop(None)
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
    /// chemin DHT, non implemente sans `dht_provider`). `hops` est le
    /// nombre de sauts du circuit choisi (`select_circuit` Python).
    pub async fn send_peers_request(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        target: Option<&IntroductionPoint>,
        hops: usize,
    ) -> Result<Vec<IntroductionPoint>, Ipv8Error> {
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
        // `target is not None and target.peer.public_key !=
        // circuit.hops[-1].public_key` Python : requete PEX via
        // `tunnel_data` vers l'IP ; sinon cellule vers la sortie
        // (chemin DHT).
        let via_tunnel = match target {
            Some(ip) => {
                let last_hop_pk = {
                    let inner = self.inner.lock().unwrap();
                    inner
                        .circuits
                        .get(&cid)
                        .and_then(|c| c.hops.last().map(|h| h.public_key_bin.clone()))
                };
                Some(&ip.peer_key) != last_hop_pk.as_ref()
            }
            None => false,
        };
        if via_tunnel {
            let ip = target.unwrap();
            let mut w = Writer::new();
            p.pack(&mut w)?;
            self.tunnel_data(
                cid,
                &ip.address,
                &pack_unsigned(&self.community_id, msg::PEERS_REQUEST, &w.into_bytes()),
            )
            .await?;
        } else {
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
        tokio::time::timeout(
            std::time::Duration::from_millis(PEERS_REQUEST_TIMEOUT_MS),
            rx,
        )
        .await
        .map_err(|_| Ipv8Error::Malformed("timeout peers-response"))?
        .map_err(|_| Ipv8Error::Malformed("cache peers abandonne"))
    }

    /// `estimate_swarm_size` pyipv8 : crawl iteratif — la requete
    /// `None` (DHT via la sortie du circuit) d'abord, puis les points
    /// d'introduction decouverts (PEX), jusqu'a
    /// `SWARM_SIZE_MAX_REQUESTS` contacts. Retourne le nombre de
    /// `seeder_pk` uniques de source `PEER_SOURCE_PEX`.
    pub async fn estimate_swarm_size(self: &Arc<Self>, info_hash: [u8; 20], hops: usize) -> usize {
        use rand::seq::SliceRandom;
        // `None` represente la requete DHT initiale.
        let mut all: HashSet<Option<IntroductionPoint>> = HashSet::from([None]);
        let mut tried: HashSet<Option<IntroductionPoint>> = HashSet::new();
        while tried.len() < SWARM_SIZE_MAX_REQUESTS {
            let mut not_tried: Vec<Option<IntroductionPoint>> =
                all.difference(&tried).cloned().collect();
            if not_tried.is_empty() {
                break;
            }
            // `random.sample` Python.
            not_tried.shuffle(&mut rand::thread_rng());
            let take = not_tried.len().min(SWARM_SIZE_MAX_REQUESTS - tried.len());
            let ips: Vec<Option<IntroductionPoint>> = not_tried.into_iter().take(take).collect();
            // `gather(..., return_exceptions=True)` : les echecs
            // n'alimentent pas `all`, seuls les succes comptent.
            let results = futures_util::future::join_all(ips.iter().map(|ip| {
                let this = self.clone();
                let ip = ip.clone();
                async move { this.send_peers_request(info_hash, ip.as_ref(), hops).await }
            }))
            .await;
            for (ip, result) in ips.into_iter().zip(results) {
                if let Ok(found) = result {
                    all.extend(found.into_iter().map(Some));
                }
                tried.insert(ip);
            }
        }
        all.iter()
            .flatten()
            .filter(|ip| ip.source == PEER_SOURCE_PEX)
            .map(|ip| ip.seeder_pk.clone())
            .collect::<HashSet<_>>()
            .len()
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
        // `elif circuit_id in self.exit_sockets` Python : a defaut de
        // store PEX local, l'EXIT fait le `find_values` DHT pour le
        // demandeur (`on_peers_request` -> `dht_lookup` ->
        // `DHTCommunityProvider.lookup`). Chemin requis pour qu'un
        // point de sortie qui n'est pas le point d'introduction puisse
        // quand meme repondre.
        let need_dht = circuit_id.is_some_and(|cid| {
            self.inner.lock().unwrap().exit_sockets.contains_key(&cid)
        });
        let dht = if need_dht { self.dht_provider() } else { None };

        let peers: Vec<tp::IntroductionInfo> = {
            let mut inner = self.inner.lock().unwrap();
            // `if info_hash in self.pex` Python : le store PEX
            // repond d'abord (nos annonces `intro_points_for`).
            if let Some(store) = inner.pex.get_mut(&p.info_hash) {
                let now = crate::pex::epoch_secs();
                let our_key = self.key.public_key().to_bin();
                store
                    .intro_points(&our_key, &my_addr, now)
                    .into_iter()
                    .take(MAX_PEERS_IN_RESPONSE)
                    .map(|ip| tp::IntroductionInfo {
                        address: ip.address,
                        key: ip.peer_key,
                        seeder_pk: ip.seeder_pk,
                        source: ip.source,
                    })
                    .collect()
            } else {
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
            }
        };
        let this = self.clone();
        let addr = UdpAddress::from(src);
        tokio::spawn(async move {
            let mut peers = peers;
            let mut lookup_done = false;
            if peers.is_empty() {
                if let Some(ref dht) = dht {
                    match dht.find_values(&p.info_hash, 0).await {
                        Ok(values) => {
                            lookup_done = true;
                            for (value, _) in values {
                                let Some(ip) = unpack_dht_intro_point(&value) else {
                                    continue;
                                };
                                peers.push(tp::IntroductionInfo {
                                    address: ip.address,
                                    key: ip.peer_key,
                                    seeder_pk: ip.seeder_pk,
                                    source: PEER_SOURCE_DHT,
                                });
                                if peers.len() >= MAX_PEERS_IN_RESPONSE {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "peers-request : find_values DHT en echec");
                        }
                    }
                }
            }
            if need_dht && dht.is_none() {
                // `unable to do a DHT lookup` Python : exit sans
                // provider -> silence cote demandeur (timeout).
                tracing::debug!(
                    info_hash = hex::encode(p.info_hash),
                    "peers-request sans provider DHT sur l'exit"
                );
            }
            // Semantique Python : `provider.lookup` renvoie None sur
            // DHTError -> aucune reponse (le demandeur timeout). Un
            // lookup reussi repond meme avec 0 pair.
            if need_dht && dht.is_some() && !lookup_done && peers.is_empty() {
                return;
            }
            let reply = tp::PeersResponse {
                circuit_id: p.circuit_id,
                identifier: p.identifier,
                info_hash: p.info_hash,
                peers,
            };
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
                        .send_to(
                            &addr,
                            &pack_unsigned(
                                &this.community_id,
                                msg::PEERS_RESPONSE,
                                &w.into_bytes(),
                            ),
                        )
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
            let n_total = p.peers.len();
            let ips: Vec<IntroductionPoint> = p
                .peers
                .into_iter()
                .filter(|i| !i.address.is_unspecified())
                .map(|i| IntroductionPoint {
                    address: i.address,
                    peer_key: i.key,
                    seeder_pk: i.seeder_pk,
                    source: i.source,
                    last_seen_secs: crate::pex::epoch_secs(),
                })
                .collect();
            tracing::info!(
                identifier = p.identifier,
                n_total,
                n_usable = ips.len(),
                info_hash = hex::encode(p.info_hash),
                "peers-response recu (intro points)"
            );
            let _ = tx.send(ips);
        }
    }

    /// `do_peer_discovery` (`hidden_services.py`) : pour chaque swarm
    /// non-seede dont le dernier lookup est assez vieux
    /// (`swarm_lookup_interval`) et qui n'a pas atteint
    /// `swarm_connection_limit`, interroge les points d'introduction
    /// connus (PEX) — ou la sortie du circuit en mode DHT
    /// (`target=None`) si aucun point n'est connu — puis lance
    /// `create_e2e` vers les points decouverts.
    pub(crate) async fn do_peer_discovery(self: &Arc<Self>) {
        // Swarms eligibles (sous verrou, sans await).
        let work: Vec<[u8; 20]> = {
            let inner = self.inner.lock().unwrap();
            inner
                .swarms
                .iter()
                .filter(|(_, s)| {
                    !s.seeding()
                        && s.last_lookup.elapsed() >= self.settings.swarm_lookup_interval
                        && s.connections
                            .keys()
                            .filter(|cid| {
                                inner.circuits.get(cid).is_some_and(|c| c.ready() && c.e2e)
                            })
                            .count()
                            < self.settings.swarm_connection_limit
                })
                .map(|(ih, _)| *ih)
                .collect()
        };
        for info_hash in work {
            self.swarm_lookup(info_hash).await;
        }
    }

    /// `Swarm.lookup` (sans `dht_provider`, les lookups "DHT" sont des
    /// `peers-request` en cellule vers la sortie — `target=None`) puis
    /// `create_e2e` vers les `seeder_pk` non connectes.
    async fn swarm_lookup(self: &Arc<Self>, info_hash: [u8; 20]) {
        let (hops, targets, dht_lookup_due) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(swarm) = inner.swarms.get_mut(&info_hash) else {
                return;
            };
            swarm.remove_old_intro_points(self.settings.swarm_max_ip_age, crate::pex::epoch_secs());
            swarm.last_lookup = Instant::now();
            let due = swarm.last_dht_response.elapsed() > self.settings.min_dht_lookup_interval
                || (swarm.intro_points.is_empty()
                    && swarm.last_dht_response.elapsed() > self.settings.max_dht_lookup_interval);
            (swarm.hops, swarm.intro_points.clone(), due)
        };

        // `gather(return_exceptions=True)` Python : les echecs sont
        // ignores, seuls les succes alimentent le swarm.
        let mut requests: Vec<Option<IntroductionPoint>> =
            targets.iter().cloned().map(Some).collect();
        // Lookup DHT si le delai le commande ou si le swarm ne connait
        // encore aucun point d'introduction. Avec un `dht_provider`
        // c'est le vrai `find_values(lookup)` sur la DHT IPv8 locale
        // (`dht_lookup` Python) ; sans provider, repli historique :
        // `peers-request` en cellule vers la sortie (`target=None`).
        let want_dht = dht_lookup_due || targets.is_empty();
        let dht = if want_dht { self.dht_provider() } else { None };
        if want_dht && dht.is_none() {
            requests.push(None);
        }
        let results = futures_util::future::join_all(requests.iter().map(|ip| {
            let this = self.clone();
            let ip = ip.clone();
            async move { this.send_peers_request(info_hash, ip.as_ref(), hops).await }
        }))
        .await;

        let mut found: Vec<IntroductionPoint> = Vec::new();
        let mut saw_dht = false;
        for ips in results.into_iter().flatten() {
            saw_dht |= ips.iter().any(|i| i.source == PEER_SOURCE_DHT);
            found.extend(ips);
        }
        // `dht_lookup` : chaque valeur stockee sous la cle du swarm
        // est un `DHTIntroPointPayload` annonce par un point
        // d'introduction.
        if let Some(dht) = dht {
            match dht.find_values(&info_hash, 0).await {
                Ok(values) => {
                    tracing::info!(
                        info_hash = hex::encode(info_hash),
                        n = values.len(),
                        "dht_lookup du swarm : valeur(s) DHT"
                    );
                    for (data, _) in values {
                        if let Some(ip) = unpack_dht_intro_point(&data) {
                            saw_dht = true;
                            found.push(ip);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "dht_lookup du swarm echoue");
                }
            }
        }
        // `add_intro_point` + `create_e2e` vers les seeder_pk non
        // connectes (`do_peer_discovery` Python).
        let pending: Vec<IntroductionPoint> = {
            let mut inner = self.inner.lock().unwrap();
            let Some(swarm) = inner.swarms.get_mut(&info_hash) else {
                return;
            };
            if saw_dht {
                swarm.last_dht_response = Instant::now();
            }
            for ip in found {
                swarm.add_intro_point(ip);
            }
            swarm
                .intro_points
                .iter()
                .filter(|ip| !swarm.has_connection(&ip.seeder_pk))
                .cloned()
                .collect()
        };
        for ip in pending {
            if let Err(e) = self.create_e2e(info_hash, &ip).await {
                tracing::debug!(error = %e, "create_e2e echoue");
            }
        }
    }

    /// `create_e2e` : envoie `create-e2e` au point d'introduction via
    /// `tunnel_data` (paquet non signe dans une cellule `data`).
    ///
    /// Retentative idempotente (`RequestCache` a retry de pyipv8) :
    /// tant qu'une requete est en cours vers ce point d'introduction,
    /// le MEME paquet est re-emis — meme `identifier`, meme cle DH —
    /// pour qu'une reponse tardive correle avec la requete en cours
    /// plutot que d'amorcer un second handshake (qui creerait un
    /// second `RP_SEEDER` cote seeder).
    pub async fn create_e2e(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        intro_point: &IntroductionPoint,
    ) -> Result<(), Ipv8Error> {
        // Re-emission tant qu'une requete est en cours : le MEME
        // `create-e2e`/`link-e2e` (identifier+DH stables) est
        // re-expedie plutot qu'un nouveau handshake. Pas d'abandon
        // temporel : une requete "abandonnee" qui aboutirait malgre
        // tout en retard cote seeder creerait un second `RP_SEEDER`
        // (l'identifiant NEUF serait traite comme une demande
        // distincte) — exactement le bug que la dedup doit eviter.
        // Un pending perime (requete consommee, liaison abandonnee
        // faute de circuit) est purge et on retombe sur un handshake
        // neuf dans le meme appel.
        loop {
            let pending = {
                let inner = self.inner.lock().unwrap();
                inner
                    .swarms
                    .get(&info_hash)
                    .and_then(|s| s.pending_e2e.get(intro_point))
                    .map(|(stage, _)| *stage)
            };
            match pending {
                // `created-e2e` recu, circuit `RP_DOWNLOADER` en
                // construction : rien a re-emettre, la liaison est
                // en cours.
                Some(PendingE2e::Building) => return Ok(()),
                Some(PendingE2e::Create(id)) | Some(PendingE2e::Link(id)) => {
                    if self.resend_e2e(info_hash, intro_point, id).await? {
                        return Ok(());
                    }
                }
                None => break,
            }
        }
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
        // "Creating e2e circuit for introduction point %s" (info).
        tracing::info!(
            info_hash = hex::encode(info_hash),
            intro = ?intro_point.address,
            circuit_id = cid,
            "create-e2e vers le point d'introduction"
        );
        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        let p = tp::CreateE2E {
            identifier,
            info_hash,
            node_public_key: intro_point.seeder_pk.clone(),
            key: dh_public.to_vec(),
        };
        let mut w = Writer::new();
        p.pack(&mut w)?;
        let packet = pack_unsigned(&self.community_id, msg::CREATE_E2E, &w.into_bytes());
        {
            let mut inner = self.inner.lock().unwrap();
            inner.e2e_requests.insert(
                identifier,
                E2ERequest {
                    info_hash,
                    dh_secret,
                    seeder_pk: intro_point.seeder_pk.clone(),
                    intro_point: intro_point.clone(),
                    packet: packet.clone(),
                },
            );
            if let Some(s) = inner.swarms.get_mut(&info_hash) {
                s.pending_e2e.insert(
                    intro_point.clone(),
                    (PendingE2e::Create(identifier), Instant::now()),
                );
            }
        }
        self.tunnel_data(cid, &intro_point.address, &packet).await
    }

    /// Re-emission de la requete e2e en cours vers un point
    /// d'introduction : le `create-e2e` d'origine tant que le
    /// `created-e2e` n'est pas arrive, sinon le `link-e2e` tant que le
    /// `linked-e2e` n'est pas arrive. Retourne `false` quand le pending
    /// est perime (requete consommee sans liaison) — il est alors
    /// purge et l'appelant ouvre un handshake neuf.
    async fn resend_e2e(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        intro_point: &IntroductionPoint,
        identifier: u16,
    ) -> Result<bool, Ipv8Error> {
        enum Stage {
            Create(Vec<u8>),
            Link { circuit_id: u32, cookie: [u8; 20] },
            Stale,
        }
        let stage = {
            let inner = self.inner.lock().unwrap();
            if let Some(req) = inner.e2e_requests.get(&identifier) {
                Stage::Create(req.packet.clone())
            } else if let Some(req) = inner.link_requests.get(&identifier) {
                Stage::Link {
                    circuit_id: req.circuit_id,
                    cookie: req.cookie,
                }
            } else {
                Stage::Stale
            }
        };
        match stage {
            Stage::Create(packet) => {
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
                self.tunnel_data(cid, &intro_point.address, &packet).await?;
                Ok(true)
            }
            Stage::Link { circuit_id, cookie } => {
                let addr = {
                    let inner = self.inner.lock().unwrap();
                    inner
                        .circuits
                        .get(&circuit_id)
                        .and_then(|c| c.first_hop().and_then(|h| h.address.clone()))
                };
                let Some(addr) = addr else {
                    return Err(Ipv8Error::Malformed("circuit e2e disparu"));
                };
                self.send_cell(
                    &addr,
                    &tp::LinkE2E {
                        circuit_id,
                        identifier,
                        cookie,
                    },
                )
                .await?;
                Ok(true)
            }
            Stage::Stale => {
                if let Some(s) = self.inner.lock().unwrap().swarms.get_mut(&info_hash) {
                    s.pending_e2e.remove(intro_point);
                }
                Ok(false)
            }
        }
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
                    // "On create-e2e: forwarding message because
                    // received over socket" (info).
                    tracing::info!(
                        circuit_id = relay_cid,
                        "create-e2e recu hors circuit : relais vers le seeder"
                    );
                    let mut w = Writer::new();
                    if p.pack(&mut w).is_err() {
                        return;
                    }
                    let packet =
                        pack_unsigned(&self.community_id, msg::CREATE_E2E, &w.into_bytes());
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
                // Dedup des retransmissions, atomique sous le lock :
                // reponse deja produite -> copie cachee ; traitement
                // en cours -> doublon ignore ; sinon on reserve la
                // cle AVANT de spawner (sinon deux create-e2e quasi
                // simultanes creeraient chacun un `RP_SEEDER`).
                enum Dedup {
                    New,
                    Busy,
                    Reply(Vec<u8>),
                }
                let requester = UdpAddress::from(src);
                let key = (p.identifier, requester.clone());
                let action = {
                    let mut inner = self.inner.lock().unwrap();
                    let Some(s) = inner.swarms.get_mut(&p.info_hash) else {
                        tracing::debug!("create-e2e recu sans swarm seeder");
                        return;
                    };
                    if s.seeder_sk.is_none() {
                        tracing::debug!("create-e2e recu sans swarm seeder");
                        return;
                    }
                    if let Some(reply) = s.seen_e2e.get(&key).cloned() {
                        Dedup::Reply(reply)
                    } else if !s.in_flight_e2e.insert(key.clone()) {
                        Dedup::Busy
                    } else {
                        Dedup::New
                    }
                };
                match action {
                    Dedup::Reply(reply) => {
                        let this = self.clone();
                        tokio::spawn(async move {
                            let _ = this.tunnel_data(cid, &requester, &reply).await;
                        });
                    }
                    Dedup::Busy => {
                        tracing::debug!("create-e2e duplique en cours, ignore");
                    }
                    Dedup::New => {
                        // "On create-e2e: creating rendezvous point"
                        // (info).
                        tracing::info!(
                            info_hash = hex::encode(p.info_hash),
                            circuit_id = cid,
                            "create-e2e recu sur circuit : creation du point de rendez-vous"
                        );
                        let info_hash = p.info_hash;
                        let this = self.clone();
                        tokio::spawn(async move {
                            this.create_created_e2e(p, requester, cid).await;
                            // La reservation expire quoi qu'il arrive :
                            // echec -> un retry ulterieur est retraite ;
                            // succes -> la reponse reste en `seen_e2e`.
                            if let Some(s) = this.inner.lock().unwrap().swarms.get_mut(&info_hash) {
                                s.in_flight_e2e.remove(&key);
                            }
                        });
                    }
                }
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
                tracing::info!(error = %e, "create_rendezvous_point echoue");
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
        tracing::debug!(
            rp_circuit = rp.circuit,
            rp_addr = ?rp_info.address,
            rp_addr_unspecified = rp_info.address.is_unspecified(),
            "created-e2e : rendezvous_info construit"
        );
        // `serializer.pack("payload", rp_info)` (NestedPayload :
        // `>H` taille + corps) — sans le prefixe, pyipv8 lit la taille
        // sur les octets de l'adresse (`Cannot unpack address type 0`).
        let Ok(rp_info_bin) = rp_info.pack_framed() else {
            return;
        };
        let Ok(rp_info_enc) = session_keys
            .clone()
            .encrypt_str(&rp_info_bin, Direction::Forward)
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
        let packet = pack_unsigned(&self.community_id, msg::CREATED_E2E, &w.into_bytes());
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(s) = inner.swarms.get_mut(&p.info_hash) {
                if s.seen_e2e.len() < MAX_SEEN_E2E {
                    s.seen_e2e
                        .insert((p.identifier, requester.clone()), packet.clone());
                }
            }
        }
        tracing::info!(
            circuit_id = intro_circuit,
            "created-e2e envoye au downloader"
        );
        let _ = self.tunnel_data(intro_circuit, &requester, &packet).await;
        // Chez pyipv8 seul le downloader marque `circuit.e2e` — le
        // seeder ne recoit pas de `linked-e2e`. Mais notre pont
        // `RP_SEEDER` -> uTP (`subscribe_circuit_data`/`pin_circuit`)
        // est installe par le listener `e2e_ready` : il faut donc le
        // declencher ici, une fois le RP cree et la reponse emise —
        // sinon la donnee e2e entrante sur le circuit RP_SEEDER
        // n'atteint jamais la lane anonyme du seeder.
        let _ = self.e2e_ready_tx.send((rp.circuit, p.info_hash));
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
        // Etape "construction" : un retry pendant la creation du
        // circuit `RP_DOWNLOADER` attend sans relancer un handshake
        // parallele ni re-emettre un `create-e2e` deja repondu. En cas
        // d'echec plus bas, le guard purge le pending pour permettre
        // une nouvelle tentative.
        let mut pending_guard = PendingGuard::new(self, &req);
        if let Some(s) = self.inner.lock().unwrap().swarms.get_mut(&req.info_hash) {
            s.pending_e2e.insert(
                req.intro_point.clone(),
                (PendingE2e::Building, Instant::now()),
            );
        }
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
        let Ok(rp_info) = tp::RendezvousInfo::unpack_framed(&mut Reader::new(&rp_bin))
        else {
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
            .discover_service(&rp_info.key, self.community_id);
        // Exclut le RP du tirage du premier saut : sinon un circuit a
        // 2 sauts pourrait choisir le RP comme premier ET dernier
        // saut (etendu vers lui-meme pour satisfaire
        // `required_exit`), ce qui corrompt l'etablissement crypto.
        let Some(first_hop) = self.pick_first_hop(Some(&rp_info.key)) else {
            return;
        };
        tracing::info!(
            info_hash = hex::encode(req.info_hash),
            rp = ?rp_info.address,
            "created-e2e valide : construction du circuit RP_DOWNLOADER"
        );
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
                tracing::info!(error = %e, "circuit RP_DOWNLOADER echoue");
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
        {
            let mut inner = self.inner.lock().unwrap();
            inner.link_requests.insert(
                identifier,
                LinkRequest {
                    circuit_id: cid,
                    info_hash: req.info_hash,
                    hs_session_keys: session_keys,
                    cookie: rp_info.cookie,
                },
            );
            // La requete progresse a l'etape `link` : `pending_e2e`
            // pointe desormais vers `link_requests` — une retentative
            // re-emettra le `link-e2e`, pas le `create-e2e`.
            if let Some(s) = inner.swarms.get_mut(&req.info_hash) {
                s.pending_e2e.insert(
                    req.intro_point.clone(),
                    (PendingE2e::Link(identifier), Instant::now()),
                );
            }
        }
        pending_guard.disarm();
        tracing::info!(circuit_id = cid, "link-e2e envoye au point de rendez-vous");
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
            // Retransmission d'un `link-e2e` deja traite : les deux
            // exits sont deja relayes — on re-repond `linked-e2e`
            // (idempotent) plutot que de laisser la requete expirer.
            if inner
                .relays
                .get(&circuit_id)
                .is_some_and(|r| r.rendezvous_relay && r.base.circuit_id == relay_cid)
            {
                true
            } else {
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
                        // Init a 1 comme `RelayRoute.__init__` Python
                        // (route creee par une cellule `relay_early`).
                        relay_early_count: 1,
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
                        relay_early_count: 1,
                    },
                );
                true
            }
        };
        if !linked {
            return;
        }
        tracing::info!(circuit_id, "circuits e2e lies au point de rendez-vous");
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
            // La liaison est faite : la requete n'est plus en cours —
            // un nouvel appel `create_e2e` pourra ouvrir un handshake
            // neuf vers ce point d'introduction.
            if let Some(s) = inner.swarms.get_mut(&req.info_hash) {
                s.pending_e2e
                    .retain(|_, (stage, _)| *stage != PendingE2e::Link(p.identifier));
            }
            if let Some(c) = inner.circuits.get_mut(&req.circuit_id) {
                c.e2e = true;
                c.hs_session_keys = Some(req.hs_session_keys);
                c.base.beat_heart();
            }
        }
        tracing::info!(
            circuit_id = req.circuit_id,
            info_hash = hex::encode(req.info_hash),
            "linked-e2e : circuit e2e pret"
        );
        let _ = self.e2e_ready_tx.send((req.circuit_id, req.info_hash));
    }
}

/// Timeout interne des sous-etapes e2e (`create_rendezvous_point`,
/// attente `READY` du RP_DOWNLOADER).
const DEFAULT_E2E_TIMEOUT_MS: u64 = 10_000;
