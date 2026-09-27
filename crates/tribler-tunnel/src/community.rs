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

use rand::RngCore;
use tribler_crypto::ipv8::dh::crypto_box_beforenm;
use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_crypto::ipv8::session::{
    crypto_auth, crypto_auth_verify, generate_session_keys, Direction, SessionKeys,
};
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::packet::{prefix_of, Packet};
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::serializer::{Reader, Writer};
use tribler_ipv8::{Ipv8Error, UdpAddress};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::cell::{self, Cell};
use crate::payload::{self as tp, msg, Cellable};
use crate::routing::{
    Circuit, Hop, RelayRoute, RoutingObject, UnverifiedHop, CIRCUIT_STATE_READY, CIRCUIT_TYPE_DATA,
    PEER_FLAG_RELAY,
};
use crate::TUNNEL_COMMUNITY_ID;

/// `max_relay_early` Python (`TunnelSettings.max_relay_early`).
const MAX_RELAY_EARLY: u8 = 8;
/// `max_joined_circuits` Python (`TunnelSettings.max_joined_circuits`).
const DEFAULT_MAX_JOINED_CIRCUITS: usize = 30;
/// Capacite du canal `data_rx` (cellules `data` livrees au
/// consommateur — SOCKS5/DHT-over-tunnel).
const DATA_CHANNEL_CAP: usize = 512;
/// `peers_list[:4]` Python : candidats relay/sortie annonces dans
/// `created`/`extended`.
const CANDIDATES_IN_RESPONSE: usize = 4;

/// Evenement "donnee recue sur un circuit" (livre au consommateur —
/// equivalent du dispatch `on_data` vers SOCKS5/services internes).
#[derive(Debug)]
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
struct Inner {
    /// `circuits` : circuits dont on est l'initiateur.
    circuits: HashMap<u32, Circuit>,
    /// `relay_from_to` : circuit_id entrant -> route sortante.
    relays: HashMap<u32, RelayRoute>,
    /// `exit_sockets` : circuit_id -> etat de sortie (hop + activation).
    exit_sockets: HashMap<u32, ExitState>,
    /// `CreateRequestCache` : identifiant du create envoye ->
    /// contexte d'extend en attente d'un `created`.
    create_requests: HashMap<u16, CreateRequest>,
    /// `CreatedRequestCache` : circuit_id -> contexte de join en
    /// attente d'un `extend` (conserve apres le premier extend).
    created_requests: HashMap<u32, CreatedRequest>,
    /// `RetryRequestCache` : circuit_id -> identifier du create/
    /// extend emis par l'initiateur.
    retry_requests: HashMap<u32, u16>,
    /// Flags de service locaux (`settings.peer_flags` : RELAY par
    /// defaut ; 0 = refuser les `create`).
    peer_flags: i32,
}

/// `TunnelExitSocket` simplifie : etat de sortie pour un circuit_id.
struct ExitState {
    /// Saut amont (pair precedent + cles de session partagees).
    hop: Hop,
    /// `enabled` : premier octet de donnee vu venant du bon IP.
    enabled: bool,
}

/// `CreateRequestCache` Python (extend en attente d'un `created`).
struct CreateRequest {
    /// `to_circuit_id` : id du `create` envoye a `to_peer`.
    to_circuit_id: u32,
    /// `from_circuit_id` : id du `extend` recu de `peer`.
    from_circuit_id: u32,
    /// `peer` : pair amont (emetteur de l'extend — cible du `extended`).
    peer: Peer,
    /// `to_peer` : pair aval (destinataire du create).
    to_peer: Peer,
    /// `extend_identifier` : identifier du `extend` recu (repercute
    /// dans le `extended`).
    extend_identifier: u16,
}

/// `CreatedRequestCache` Python (join en attente d'un `extend`).
struct CreatedRequest {
    /// Pair amont.
    peer: Peer,
    /// Candidats proposes (`peers_dict` : pubkey_bin -> Peer).
    candidates: HashMap<Vec<u8>, Peer>,
}

/// `TunnelCommunity`.
pub struct TunnelCommunity {
    /// Identite locale (LibNaCL).
    key: LibNaClSecretKey,
    /// Annuaire reseau partage.
    network: Arc<Network>,
    /// Endpoint UDP partage.
    endpoint: Arc<UdpEndpoint>,
    /// Etat interne.
    inner: Mutex<Inner>,
    /// Compteur `identifier` des requetes (`number` du RequestCache).
    identifier: AtomicU16,
    /// `global_time` (Lamport local pour les paquets signes, ex.
    /// `destroy`).
    global_time: AtomicU64,
    /// Cellules `data` livrees au consommateur.
    data_tx: tokio::sync::mpsc::Sender<CircuitData>,
    /// Receveur cote consommateur (pris une fois par `data_rx()`).
    data_rx: Mutex<Option<tokio::sync::mpsc::Receiver<CircuitData>>>,
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
        let (data_tx, data_rx) = tokio::sync::mpsc::channel(DATA_CHANNEL_CAP);
        let community = Arc::new(Self {
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
                peer_flags,
            }),
            identifier: AtomicU16::new(0),
            global_time: AtomicU64::new(0),
            data_tx,
            data_rx: Mutex::new(Some(data_rx)),
        });
        let weak = Arc::downgrade(&community);
        endpoint
            .add_raw_prefix_listener(
                prefix_of(&TUNNEL_COMMUNITY_ID),
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
    fn next_id(&self) -> u16 {
        self.identifier.fetch_add(1, Ordering::Relaxed)
    }

    /// `claim_global_time` pour les paquets signes hors cellule.
    fn claim_global_time(&self) -> u64 {
        self.global_time.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Receveur des donnees de circuit (un seul consommateur — le
    /// premier appel prend le receveur, les suivants obtiennent
    /// `None`).
    pub fn data_rx(&self) -> Option<tokio::sync::mpsc::Receiver<CircuitData>> {
        self.data_rx.lock().unwrap().take()
    }

    /// Nombre de circuits initiates connus.
    pub fn circuit_count(&self) -> usize {
        self.inner.lock().unwrap().circuits.len()
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
    async fn send_cell<P: Cellable>(&self, addr: &UdpAddress, p: &P) -> Result<(), Ipv8Error> {
        let mut w = Writer::new();
        p.pack(&mut w)?;
        let body = w.into_bytes();
        if body.len() < 4 {
            return Err(Ipv8Error::Malformed("cellable sans circuit_id"));
        }
        let circuit_id = u32::from_be_bytes(body[..4].try_into().unwrap());
        let plaintext = cell::NO_CRYPTO_PACKETS.contains(&P::MSG_ID);
        // `cell.message` Python = `inner_msg_id + payload` (le
        // `circuit_id` figure deux fois : en-tete de cellule ET
        // premier champ du payload). On construit la cellule en clair
        // puis `encrypt_cell` chiffre `cell[29..]` par couches.
        let mut relay_early = false;
        let mut crypto: Option<(Direction, Vec<SessionKeys>)> = None;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(circuit) = inner.circuits.get_mut(&circuit_id) {
                relay_early =
                    P::MSG_ID == msg::EXTEND || circuit.relay_early_count < MAX_RELAY_EARLY;
                if relay_early {
                    circuit.relay_early_count += 1;
                }
                if !plaintext {
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
                    // Route de retour : chiffre dans la direction de
                    // l'AUTRE route (`other.direction`, `other.hop`).
                    if let Some(other) = inner.relays.get(&relay.base.circuit_id) {
                        crypto = Some((other.direction, vec![other.hop.session_keys.clone()]));
                    }
                }
            }
        }

        let mut wire = Cell::to_wire(
            &prefix_of(&TUNNEL_COMMUNITY_ID),
            circuit_id,
            P::MSG_ID,
            &body,
            plaintext,
            relay_early,
        );
        if let Some((dir, mut keys)) = crypto {
            wire = cell::encrypt_cell(&wire, dir, &mut keys)?;
        }
        self.endpoint.send_to(addr, &wire).await
    }

    /// `create_circuit` : cree un circuit de `goal_hops` sauts dont le
    /// premier hop est `first_hop`. Retourne le `circuit_id`.
    pub async fn create_circuit(
        self: &Arc<Self>,
        goal_hops: usize,
        first_hop: &Peer,
    ) -> Result<u32, Ipv8Error> {
        let circuit_id = self.gen_circuit_id();
        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        let addr = first_hop
            .address
            .clone()
            .ok_or(Ipv8Error::Malformed("hop sans adresse"))?;
        {
            let mut inner = self.inner.lock().unwrap();
            let mut circuit = Circuit::new(circuit_id, goal_hops, CIRCUIT_TYPE_DATA, None);
            circuit.unverified_hop = Some(UnverifiedHop {
                public_key_bin: first_hop.public_key_bin.clone(),
                address: Some(addr.clone()),
                dh_secret,
                identifier,
            });
            inner.circuits.insert(circuit_id, circuit);
            inner.retry_requests.insert(circuit_id, identifier);
        }
        let create = tp::Create {
            circuit_id,
            identifier,
            node_public_key: self.key.public_key().to_bin(),
            key: dh_public.to_vec(),
        };
        self.send_cell(&addr, &create).await?;
        Ok(circuit_id)
    }

    /// `send_extend` : envoie un `ExtendPayload` chiffre au premier
    /// saut du circuit pour ajouter `extend_with`.
    async fn send_extend(
        self: &Arc<Self>,
        circuit_id: u32,
        extend_with: &Peer,
        candidate_keys: &[Vec<u8>],
    ) -> Result<(), Ipv8Error> {
        let (dh_secret, dh_public) = generate_diffie_secret();
        let identifier = self.next_id();
        let (first_hop_addr, node_addr) = {
            let mut inner = self.inner.lock().unwrap();
            let first = {
                let Some(c) = inner.circuits.get_mut(&circuit_id) else {
                    return Err(Ipv8Error::Malformed("circuit inconnu"));
                };
                c.unverified_hop = Some(UnverifiedHop {
                    public_key_bin: extend_with.public_key_bin.clone(),
                    address: extend_with.address.clone(),
                    dh_secret,
                    identifier,
                });
                c.first_hop()
                    .and_then(|h| h.address.clone())
                    .ok_or(Ipv8Error::Malformed("pas de premier hop"))?
            };
            inner.retry_requests.insert(circuit_id, identifier);
            // `node_addr` : adresse publique du candidat quand elle
            // n'etait pas dans les candidats proposes (0.0.0.0:0 sinon
            // — le dernier hop resoudra via `request.candidates`).
            let node_addr = if candidate_keys.contains(&extend_with.public_key_bin) {
                UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap())
            } else {
                extend_with
                    .address
                    .clone()
                    .unwrap_or_else(|| UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap()))
            };
            (first, node_addr)
        };
        let p = tp::Extend {
            circuit_id,
            identifier,
            node_public_key: extend_with.public_key_bin.clone(),
            key: dh_public.to_vec(),
            node_addr,
        };
        self.send_cell(&first_hop_addr, &p).await
    }

    /// Point d'entree brut (raw prefix listener) : cellule ou paquet.
    pub fn on_raw_datagram(self: &Arc<Self>, src: SocketAddr, data: &[u8]) {
        let prefix = prefix_of(&TUNNEL_COMMUNITY_ID);
        if cell::is_cell(&prefix, data) {
            if let Err(e) = self.process_cell(src, data) {
                tracing::debug!(error = %e, "cellule rejetee");
            }
            return;
        }
        match Packet::parse(data, Some(&TUNNEL_COMMUNITY_ID)) {
            Ok(pkt) => {
                if let Err(e) = self.on_packet(src, pkt) {
                    tracing::debug!(error = %e, "paquet tunnel rejete");
                }
            }
            Err(e) => tracing::trace!(error = %e, "datagramme ignore"),
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
            Circuit(Vec<SessionKeys>),
        }
        let crypto = {
            let inner = self.inner.lock().unwrap();
            if let Some(exit) = inner.exit_sockets.get(&circuit_id) {
                Crypto::Exit(exit.hop.session_keys.clone())
            } else if let Some(c) = inner.circuits.get(&circuit_id) {
                Crypto::Circuit(c.hops.iter().map(|h| h.session_keys.clone()).collect())
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
            Crypto::Circuit(ks) => cell::decrypt_cell(data, Direction::Backward, &ks)?,
        };

        // Checks de flags post-decrypt (comme `process_cell` Python).
        cell::check_cell_flags(&decrypted, MAX_RELAY_EARLY)?;
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
        if parsed.relay_early && route.relay_early_count >= MAX_RELAY_EARLY {
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
    fn on_cell_message(self: &Arc<Self>, src: SocketAddr, cell: &Cell) -> Result<(), Ipv8Error> {
        let mut r = Reader::new(&cell.message);
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
            _ => {
                tracing::trace!(msg_id = cell.inner_msg_id, "cellule ignoree");
            }
        }
        Ok(())
    }

    /// Paquet IPv8 (signe ou non) recu sur le prefixe tunnel :
    /// `destroy` et messages E2E (non-cellules).
    fn on_packet(&self, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        match pkt.msg_id {
            msg::DESTROY => {
                let mut r = Reader::new(&pkt.payload);
                let d = tp::Destroy::unpack(&mut r)?;
                self.on_destroy(src, d.circuit_id, d.reason);
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
            || joined >= DEFAULT_MAX_JOINED_CIRCUITS
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
    async fn join_circuit(&self, src: SocketAddr, p: tp::Create) -> Result<(), Ipv8Error> {
        let circuit_id = p.circuit_id;
        let ds = generate_diffie_shared_secret(&p.key, &self.key)?;
        let session_keys = generate_session_keys(&ds.shared)?;

        let requester = Peer::new(p.node_public_key.clone(), Some(UdpAddress::from(src)))
            .ok_or(Ipv8Error::Malformed("cle de requete invalide"))?;

        // Candidats proposes : relays puis sorties (marqueur = premiere
        // sortie dupliquee — convention `join_circuit` Python).
        let peers = self.network.peers_for_service(&TUNNEL_COMMUNITY_ID);
        let (exits, relays_only): (Vec<Peer>, Vec<Peer>) = peers
            .into_iter()
            .filter(|q| q.public_key_bin != requester.public_key_bin)
            .partition(q_is_exit_candidate);
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
                },
            );
        }

        let reply = tp::Created {
            circuit_id,
            identifier: p.identifier,
            key: ds.crypt_pk.to_vec(),
            auth: ds.auth,
            candidates_enc,
        };
        self.send_cell(&UdpAddress::from(src), &reply).await
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
                .copied()
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
                .copied()
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
            let Some(circuit) = inner.circuits.get_mut(&circuit_id) else {
                return;
            };
            circuit.add_hop(Hop {
                public_key_bin: hop_pk_bin,
                address: hop_addr,
                session_keys,
            });
            let goal = circuit.goal_hops;
            let done = circuit.hops.len();
            (goal, done, goal.saturating_sub(1) == done)
        };

        if hops_done < goal {
            // Choix du prochain candidat : sorties si le prochain hop
            // est le dernier, sinon relays (fallback sorties).
            let wanted: &[Vec<u8>] = if become_exit || relay_keys.is_empty() {
                exit_keys
            } else {
                relay_keys
            };
            let next = wanted
                .iter()
                .find_map(|pk| self.network.get_by_key(pk))
                .or_else(|| {
                    self.network
                        .peers_for_service(&TUNNEL_COMMUNITY_ID)
                        .into_iter()
                        .find(|q| {
                            let inner = self.inner.lock().unwrap();
                            inner
                                .circuits
                                .get(&circuit_id)
                                .map(|c| {
                                    !c.hops.iter().any(|h| h.public_key_bin == q.public_key_bin)
                                })
                                .unwrap_or(false)
                        })
                });
            let cand_list: Vec<Vec<u8>> = cand_keys.clone();
            if let Some(next_peer) = next {
                let c = self.clone();
                tokio::spawn(async move {
                    let _ = c.send_extend(circuit_id, &next_peer, &cand_list).await;
                });
            }
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
    fn on_data(&self, src: SocketAddr, circuit_id: u32, p: tp::Data) {
        let is_exit = {
            self.inner
                .lock()
                .unwrap()
                .exit_sockets
                .contains_key(&circuit_id)
        };
        if is_exit {
            self.exit_data(circuit_id, src, &p);
        } else {
            let _ = self.data_tx.try_send(CircuitData {
                circuit_id,
                source: src,
                destination: p.dest_address,
                origin: p.org_address,
                data: p.data,
            });
        }
    }

    /// `exit_data` : activation au premier octet vu du bon IP puis
    /// envoi UDP brut vers la destination.
    fn exit_data(&self, circuit_id: u32, src: SocketAddr, p: &tp::Data) {
        {
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
        }
        // TODO(etape 12) : socket de sortie UDP dediee (TunnelExitSocket)
        // pour recevoir les reponses hors-prefixe et les renvoyer dans
        // le tunnel. Le "spoke" reutilise l'endpoint pour l'instant.
        tracing::trace!(circuit_id, dest = ?p.dest_address, "exit_data");
        let ep = self.endpoint.clone();
        let dest = p.dest_address.clone();
        let data = p.data.clone();
        tokio::spawn(async move {
            let _ = ep.send_to(&dest, &data).await;
        });
    }

    /// `on_destroy` : nettoie circuit/relais/sortie concernes.
    fn on_destroy(&self, _src: SocketAddr, circuit_id: u32, reason: u16) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(route) = inner.relays.remove(&circuit_id) {
            inner.relays.remove(&route.base.circuit_id);
        }
        inner.exit_sockets.remove(&circuit_id);
        inner.circuits.remove(&circuit_id);
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
        let pkt = Packet::sign(
            &TUNNEL_COMMUNITY_ID,
            msg::DESTROY,
            &self.key,
            self.claim_global_time(),
            &w.into_bytes(),
        );
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
        self.send_cell(&addr, &p).await
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
        self.send_cell(&addr, &ping).await
    }
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
struct DiffieShared {
    /// Secret partage `s1 || s2` (64 octets).
    shared: [u8; 64],
    /// `crypt_pk` de la cle ephemere repondante (envoye dans `key`).
    crypt_pk: [u8; 32],
    /// `auth` = `crypto_auth(shared[:32], crypt_pk)`.
    auth: [u8; 32],
}

/// `generate_diffie_shared_secret` (hop) : `shared = DH(tmp2, dh) +
/// DH(node_sk, dh)` ; retourne le secret partage, la cle publique
/// ephemere et son auth.
fn generate_diffie_shared_secret(
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
fn verify_and_generate_shared_secret(
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

/// Heuristique "candidat sortie" (approximation de
/// `get_candidates(PEER_FLAG_EXIT_BT)` : le suivi des flags distants
/// par pair n'est pas encore cable dans `Network` — aucun pair n'est
/// marque sortie, les listes de candidats ne portent donc que des
/// relais pour l'instant).
fn q_is_exit_candidate(_p: &Peer) -> bool {
    false
}
