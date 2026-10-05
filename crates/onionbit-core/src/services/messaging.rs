// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Service de messagerie anonyme (ADR-0011, Phase 8 etape 37) —
//! orchestration : jointure du swarm de presence, resolution des
//! points d'introduction, liaison e2e, demultiplexage des cellules
//! `data` par swarm messagerie, emission.
//!
//! Le **codec** et la **crypto applicative** vivent dans
//! `onionbit-messaging` (proprietaire unique du protocole) ; ce
//! service ne fait que brancher le protocole sur les circuits e2e
//! de `TunnelCommunity`.
//!
//! Separation des lanes : un swarm messagerie
//! (`messaging_hash(pk)`) n'apparait JAMAIS dans `swarm_lookup`
//! (mapping BitTorrent `lookup -> download`) — le listener e2e de
//! `ipv8_stack` ignore donc nos circuits et l'injection uTP ne peut
//! pas recevoir de trame messagerie (MS-9).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use onionbit_ipv8::UdpAddress;
use onionbit_messaging::{
    derive_messaging_keys, messaging_hash, Frame, MessagingConfig, MessagingError, MessagingKeys,
    MsgKind, RawFrame, RecvWindow,
};
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::IntroductionPoint;
use tokio::sync::{broadcast, watch};

use crate::error::{CoreError, Result};

/// Capacite du canal d'evenements applicatifs (bound interne — les
/// consommateurs lents perdent des evenements, jamais le service).
const EVENTS_CAP: usize = 256;
/// Timeout d'attente de liaison e2e dans `connect`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Granularite de scrutation de la liaison dans `connect`.
const CONNECT_POLL: Duration = Duration::from_millis(50);

/// Secondes Unix courantes.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Adresse non definie `0.0.0.0:0` — `dest`/`origin` des cellules
/// `data` messagerie : le relais de rendez-vous n'interprete pas ces
/// champs (contrairement aux sorties UDP), et la trame ne pretend
/// a aucune forme applicative connue (jamais uTP).
fn unspecified() -> UdpAddress {
    UdpAddress::Ipv4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// Evenement applicatif emis par le service (futur relais API/SSE,
/// etape 40 — et oracle des bancs MS-*).
#[derive(Debug, Clone)]
pub enum MessagingEvent {
    /// Trame authentifiee admise : codec + signature Ed25519 +
    /// dechiffrement + anti-replay ont tous passe.
    Frame {
        /// `pk_bin` de l'emetteur verifie.
        contact: Vec<u8>,
        /// Type de trame.
        kind: MsgKind,
        /// `id` de trame (cible des `ack`, etape 39).
        id: [u8; 16],
        /// Corps applicatif en clair.
        body: Vec<u8>,
    },
    /// Circuit e2e lie a un contact connu (initiateur, ou repondant
    /// resolu par une premiere trame verifiee).
    Bound {
        /// `pk_bin` du contact.
        contact: Vec<u8>,
        /// `circuit_id` lie.
        circuit_id: u32,
    },
    /// Circuit repondant lie (`RP_SEEDER`) — l'expediteur n'est pas
    /// encore identifie ; il le sera a la premiere trame verifiee.
    Pending {
        /// `circuit_id` lie.
        circuit_id: u32,
    },
}

/// Etat d'un contact (cle : `pk_bin`).
struct Contact {
    /// Cle publique du contact (verification des signatures).
    pk: LibNaClPublicKey,
    /// Fenetre anti-replay entrante (persiste d'un circuit a l'autre
    /// — une re-emission honnete apres reouverture est absorbee par
    /// la dedup `id`, MS-2).
    recv_window: RecvWindow,
    /// Prochain `seq` sortant (monotone par direction).
    send_seq: u64,
    /// Circuit e2e lie a ce contact (`None` = a etablir).
    circuit: Option<u32>,
    /// `hello` deja emis sur le circuit lie.
    greeted: bool,
}

/// Liaison d'un circuit e2e a la messagerie.
struct CircuitBinding {
    /// Cles applicatives HKDF de cette liaison (`send`/`recv` selon
    /// notre role — initiateur ou repondant).
    keys: MessagingKeys,
    /// `pk_bin` du contact si connu : l'initiateur le sait des la
    /// liaison (le swarm cible porte `messaging_hash(pk)`), le
    /// repondant l'apprend a la premiere trame verifiee.
    contact: Option<Vec<u8>>,
}

/// Service de messagerie e2e (une instance par `Ipv8Stack`).
pub struct MessagingService {
    /// Community tunnel — transport uniquement.
    tunnel: Arc<TunnelCommunity>,
    /// Cle d'identite du demon (signature des trames + `seeder_sk`
    /// du swarm de presence : le `seeder_pk` publie est notre `pk`,
    /// donc le handshake e2e authentifie le destinataire).
    key: LibNaClSecretKey,
    /// Bornes du protocole (`onionbit-messaging`).
    cfg: MessagingConfig,
    /// Sauts des circuits messagerie (`hops` de `join_swarm`).
    hops: usize,
    /// `messaging_hash` de notre propre swarm de presence.
    own_mh: [u8; 20],
    /// Swarms de contacts joints : `messaging_hash(pk)` -> `pk_bin`.
    swarms: Mutex<HashMap<[u8; 20], Vec<u8>>>,
    /// Etat par contact (`pk_bin`).
    contacts: Mutex<HashMap<Vec<u8>, Contact>>,
    /// Liaisons par `circuit_id`.
    circuits: Mutex<HashMap<u32, CircuitBinding>>,
    /// Emetteur d'evenements applicatifs.
    events_tx: broadcast::Sender<MessagingEvent>,
    /// Arrets des taches du service.
    stops: Mutex<Vec<watch::Sender<bool>>>,
}

impl MessagingService {
    /// Demarre le service : joint le swarm de presence
    /// (`messaging_hash(own_pk)`, `seeder_sk` = cle d'identite),
    /// lance le relais `e2e_ready` et le moniteur de presence
    /// (points d'introduction + re-annonce DHT).
    ///
    /// `hops` = sauts des circuits `IP_SEEDER`/`RP_DOWNLOADER`
    /// (hors +1 swarm automatique — comme les `anon_hops` BT).
    pub fn start(
        tunnel: Arc<TunnelCommunity>,
        key: LibNaClSecretKey,
        cfg: MessagingConfig,
        hops: usize,
    ) -> Arc<Self> {
        let own_mh = messaging_hash(&key.public_key());
        // `seeder_sk` = cle d'identite : le `seeder_pk` annonce par
        // les points d'introduction est notre `pk` publique — le
        // `create-e2e` d'un expéditeur verifie `auth` contre elle
        // (authentification transport DU destinataire ; le sens
        // inverse est prouve par les signatures de trames).
        tunnel.join_swarm_with_key(own_mh, hops, Some(key.clone()));
        let (events_tx, _) = broadcast::channel(EVENTS_CAP);
        let svc = Arc::new(Self {
            tunnel,
            key,
            cfg,
            hops,
            own_mh,
            swarms: Mutex::new(HashMap::new()),
            contacts: Mutex::new(HashMap::new()),
            circuits: Mutex::new(HashMap::new()),
            events_tx,
            stops: Mutex::new(Vec::new()),
        });
        svc.spawn_e2e_listener();
        svc.spawn_presence_monitor();
        svc
    }

    /// Abonne un receveur aux evenements applicatifs.
    pub fn subscribe(&self) -> broadcast::Receiver<MessagingEvent> {
        self.events_tx.subscribe()
    }

    /// Arret du service : quitte les swarms messagerie et stoppe les
    /// taches (les circuits e2e meurent avec leurs circuits tunnels).
    pub fn stop(&self) {
        self.tunnel.leave_swarm(&self.own_mh);
        let mh: Vec<[u8; 20]> = self.swarms.lock().unwrap().keys().copied().collect();
        for m in mh {
            self.tunnel.leave_swarm(&m);
        }
        for tx in self.stops.lock().unwrap().drain(..) {
            let _ = tx.send(true);
        }
    }

    /// Resout les points d'introduction d'un contact via le chemin
    /// DHT de la sortie d'un circuit (`send_peers_request(None)`).
    /// Joint le swarm du contact en downloader si besoin.
    pub async fn resolve(&self, contact_pk: &[u8]) -> Result<Vec<IntroductionPoint>> {
        let pk = LibNaClPublicKey::from_bin(contact_pk)
            .map_err(|e| CoreError::State(format!("cle de contact invalide: {e}")))?;
        let mh = self.ensure_contact_swarm(&pk, contact_pk);
        self.tunnel
            .send_peers_request(mh, None, self.hops)
            .await
            .map_err(|e| CoreError::State(format!("peers-request messagerie: {e}")))
    }

    /// Lie un circuit e2e vers un contact via un point
    /// d'introduction (chemin PEX, ou un IP resolu par [`Self::resolve`]).
    /// Retourne le `circuit_id` lie.
    pub async fn connect(&self, contact_pk: &[u8], intro: &IntroductionPoint) -> Result<u32> {
        let pk = LibNaClPublicKey::from_bin(contact_pk)
            .map_err(|e| CoreError::State(format!("cle de contact invalide: {e}")))?;
        let mh = self.ensure_contact_swarm(&pk, contact_pk);
        // Circuit deja lie -> rien a faire (idempotent).
        if let Some(cid) = self.contact_circuit(contact_pk) {
            return Ok(cid);
        }
        self.tunnel
            .create_e2e(mh, intro)
            .await
            .map_err(|e| CoreError::State(format!("create-e2e messagerie: {e}")))?;
        // Le relais `e2e_ready` pose la liaison de facon asynchrone :
        // scruter `contacts[pk].circuit` jusqu'au timeout.
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            if let Some(cid) = self.contact_circuit(contact_pk) {
                return Ok(cid);
            }
            if std::time::Instant::now() >= deadline {
                return Err(CoreError::State("timeout de liaison e2e messagerie".into()));
            }
            tokio::time::sleep(CONNECT_POLL).await;
        }
    }

    /// Envoie un message applicatif a un contact dont le circuit est
    /// lie (voir [`Self::connect`]). `NotConnected` (InvalidState) si
    /// aucun circuit — la resolution/liaison explicite reste a la
    /// charge de l'appelant (online-only : pas de file).
    ///
    /// Un `hello` (corps = notre `pk_bin`) precede le premier
    /// message sur chaque liaison : c'est la trame qui identifie
    /// l'expediteur cote repondant (consentement = etape 38).
    pub async fn send(&self, contact_pk: &[u8], body: Vec<u8>) -> Result<[u8; 16]> {
        let cid = self
            .contact_circuit(contact_pk)
            .ok_or(CoreError::InvalidState("messagerie : contact non lie"))?;
        if !self.is_greeted(contact_pk) {
            self.send_frame(
                contact_pk,
                cid,
                MsgKind::Hello,
                self.key.public_key().to_bin(),
            )
            .await?;
        }
        self.send_frame(contact_pk, cid, MsgKind::Msg, body).await
    }

    /// Snapshot diagnostic : `(pk_bin, circuit lie)` par contact.
    pub fn bound_contacts(&self) -> Vec<(Vec<u8>, Option<u32>)> {
        self.contacts
            .lock()
            .unwrap()
            .iter()
            .map(|(pk, c)| (pk.clone(), c.circuit))
            .collect()
    }

    /// `circuit_id` lie du contact, s'il existe.
    fn contact_circuit(&self, contact_pk: &[u8]) -> Option<u32> {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .and_then(|c| c.circuit)
    }

    /// `hello` deja emis pour ce contact.
    fn is_greeted(&self, contact_pk: &[u8]) -> bool {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .is_some_and(|c| c.greeted)
    }

    /// Joint le swarm du contact en downloader (`seeder_sk = None`)
    /// et cree l'etat de contact ; retourne le `messaging_hash`.
    fn ensure_contact_swarm(&self, pk: &LibNaClPublicKey, pk_bin: &[u8]) -> [u8; 20] {
        let mh = messaging_hash(pk);
        {
            let mut swarms = self.swarms.lock().unwrap();
            if let std::collections::hash_map::Entry::Vacant(e) = swarms.entry(mh) {
                e.insert(pk_bin.to_vec());
                self.tunnel.join_swarm(mh, self.hops, false);
            }
        }
        self.contacts
            .lock()
            .unwrap()
            .entry(pk_bin.to_vec())
            .or_insert_with(|| Contact {
                pk: pk.clone(),
                recv_window: RecvWindow::new(&self.cfg),
                send_seq: 0,
                circuit: None,
                greeted: false,
            });
        mh
    }

    /// Scelle et emet une trame sur le circuit du contact.
    async fn send_frame(
        &self,
        contact_pk: &[u8],
        cid: u32,
        kind: MsgKind,
        body: Vec<u8>,
    ) -> Result<[u8; 16]> {
        let (send_key, seq) = {
            let circuits = self.circuits.lock().unwrap();
            let mut contacts = self.contacts.lock().unwrap();
            let (Some(binding), Some(contact)) = (circuits.get(&cid), contacts.get_mut(contact_pk))
            else {
                return Err(CoreError::InvalidState("messagerie : liaison disparue"));
            };
            let seq = contact.send_seq;
            contact.send_seq += 1;
            (binding.keys.send, seq)
        };
        let frame = Frame::new(kind, seq, now_secs(), body);
        let wire = frame
            .seal(&self.key, &send_key, &self.cfg)
            .map_err(|e| CoreError::State(format!("seal trame: {e}")))?;
        if let Err(e) = self
            .tunnel
            .send_data(cid, &unspecified(), &unspecified(), &wire)
            .await
        {
            // Le circuit est mort entre-temps : delier pour que le
            // prochain envoi retente une liaison.
            self.unbind_circuit(cid);
            return Err(CoreError::State(format!("send_data messagerie: {e}")));
        }
        if kind == MsgKind::Hello {
            if let Some(c) = self.contacts.lock().unwrap().get_mut(contact_pk) {
                c.greeted = true;
            }
        }
        Ok(frame.id)
    }

    /// Relais `e2e_ready` : demultiplexe les liaisons par swarm —
    /// `own_mh` = repondant (contact a identifier), swarm contact =
    /// initiateur (contact connu). Les swarms BitTorrent sont
    /// ignores (listener dedie dans `ipv8_stack`).
    fn spawn_e2e_listener(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops.lock().unwrap().push(tx);
        let mut e2e = self.tunnel.e2e_ready();
        let svc = self.clone();
        tokio::spawn(async move {
            loop {
                let (cid, lookup) = tokio::select! {
                    _ = rx_stop.changed() => break,
                    ev = e2e.recv() => match ev {
                        Ok(v) => v,
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    },
                };
                svc.on_e2e_ready(cid, lookup);
            }
        });
    }

    /// Liaison e2e vers un swarm messagerie : derive les cles
    /// applicatives, abonne les donnees du circuit et lance la
    /// reception.
    fn on_e2e_ready(self: &Arc<Self>, cid: u32, lookup: [u8; 20]) {
        // Ctype -> role : `RP_DOWNLOADER` = initiateur du lien e2e.
        let ctype = self
            .tunnel
            .circuits_info()
            .into_iter()
            .find(|c| c.circuit_id == cid)
            .map(|c| c.ctype)
            .unwrap_or_default();
        let (initiator, contact) = if lookup == self.own_mh {
            // Un pair nous contacte : toujours repondant.
            (false, None)
        } else if let Some(pk_bin) = self.swarms.lock().unwrap().get(&lookup) {
            (
                ctype == onionbit_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER,
                Some(pk_bin.clone()),
            )
        } else {
            // Swarm non messagerie (BitTorrent ou autre) : pas pour nous.
            return;
        };
        let Some(shared) = self.tunnel.e2e_shared_secret(cid) else {
            return;
        };
        let Ok(keys) = derive_messaging_keys(&shared, initiator) else {
            return;
        };
        self.circuits.lock().unwrap().insert(
            cid,
            CircuitBinding {
                keys: keys.clone(),
                contact: contact.clone(),
            },
        );
        match contact {
            Some(pk_bin) => {
                if let Some(c) = self.contacts.lock().unwrap().get_mut(&pk_bin) {
                    c.circuit = Some(cid);
                }
                let _ = self.events_tx.send(MessagingEvent::Bound {
                    contact: pk_bin,
                    circuit_id: cid,
                });
            }
            None => {
                let _ = self
                    .events_tx
                    .send(MessagingEvent::Pending { circuit_id: cid });
            }
        }
        // Reception des trames sur ce circuit (abonne dedie — le
        // subscriber BitTorrent ne s'abonne jamais a nos circuits).
        let rx = self.tunnel.subscribe_circuit_data(cid);
        let svc = self.clone();
        tokio::spawn(async move {
            svc.recv_circuit(cid, keys, rx).await;
        });
    }

    /// Boucle de reception d'un circuit : parse chaque `data`,
    /// identifie l'emetteur si besoin, verifie, anti-replay, livre.
    async fn recv_circuit(
        self: &Arc<Self>,
        cid: u32,
        keys: MessagingKeys,
        mut rx: tokio::sync::mpsc::Receiver<onionbit_tunnel::community::CircuitData>,
    ) {
        while let Some(msg) = rx.recv().await {
            self.handle_incoming(cid, &keys, &msg.data);
        }
        // Canal ferme = circuit detruit : nettoyer la liaison.
        self.unbind_circuit(cid);
    }

    /// Delie un circuit (mort ou erreur d'emission) sans toucher la
    /// fenetre anti-replay du contact — elle survit a la
    /// reouverture (MS-2).
    fn unbind_circuit(&self, cid: u32) {
        let contact = self
            .circuits
            .lock()
            .unwrap()
            .remove(&cid)
            .and_then(|b| b.contact);
        if let Some(pk_bin) = contact {
            if let Some(c) = self.contacts.lock().unwrap().get_mut(&pk_bin) {
                if c.circuit == Some(cid) {
                    c.circuit = None;
                }
            }
        }
    }

    /// Codec + identification + verification + anti-replay d'une
    /// trame entrante. Ne panique jamais ; toute trame hostile est
    /// ecartee en silence (debug).
    fn handle_incoming(&self, cid: u32, keys: &MessagingKeys, data: &[u8]) {
        let raw = match RawFrame::parse(data, &keys.recv, &self.cfg) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(circuit_id = cid, error = %e, "trame messagerie rejetee au codec");
                return;
            }
        };
        let bound = self
            .circuits
            .lock()
            .unwrap()
            .get(&cid)
            .and_then(|b| b.contact.clone());
        let pk_bin = match bound {
            // Initiateur (ou repondant deja resolu) : une seule cle.
            Some(pk_bin) => {
                let Some(Ok(_)) = self.verify_against(&pk_bin, &raw) else {
                    tracing::debug!(circuit_id = cid, "signature de trame invalide");
                    return;
                };
                pk_bin
            }
            // Repondant non resolu : `hello` declare l'emetteur
            // (corps = sa `pk_bin`), sinon balayage des contacts
            // connus (reouverture de circuit par un contact existant).
            None => match self.identify(&raw) {
                Some(pk) => {
                    self.bind_contact(cid, &pk);
                    let _ = self.events_tx.send(MessagingEvent::Bound {
                        contact: pk.clone(),
                        circuit_id: cid,
                    });
                    pk
                }
                None => {
                    tracing::debug!(
                        circuit_id = cid,
                        "trame d'emetteur non identifiable, ignoree"
                    );
                    return;
                }
            },
        };
        // Anti-replay apres preuve d'authenticite.
        let admitted = {
            let mut contacts = self.contacts.lock().unwrap();
            contacts
                .get_mut(&pk_bin)
                .map(|c| c.recv_window.admit(raw.seq, &raw.id))
        };
        match admitted {
            Some(Ok(())) => {
                let _ = self.events_tx.send(MessagingEvent::Frame {
                    contact: pk_bin,
                    kind: raw.kind,
                    id: raw.id,
                    body: raw.body,
                });
            }
            Some(Err(e)) => {
                tracing::debug!(circuit_id = cid, error = %e, "trame messagerie rejetee (anti-replay)");
            }
            None => {}
        }
    }

    /// Verifie la trame contre la cle du contact `pk_bin`.
    fn verify_against(
        &self,
        pk_bin: &[u8],
        raw: &RawFrame,
    ) -> Option<std::result::Result<Frame, MessagingError>> {
        self.contacts
            .lock()
            .unwrap()
            .get(pk_bin)
            .map(|c| raw.verify(&c.pk))
    }

    /// Identifie l'emetteur d'une trame sur un circuit non lie :
    /// `hello` (corps = `pk_bin`) d'abord, sinon chaque contact
    /// connu jusqu'a la premiere verification reussie.
    fn identify(&self, raw: &RawFrame) -> Option<Vec<u8>> {
        if raw.kind == MsgKind::Hello {
            if let Ok(pk) = LibNaClPublicKey::from_bin(&raw.body) {
                if raw.verify(&pk).is_ok() {
                    return Some(pk.to_bin());
                }
            }
        }
        let pks: Vec<Vec<u8>> = self.contacts.lock().unwrap().keys().cloned().collect();
        pks.into_iter()
            .find(|pk_bin| matches!(self.verify_against(pk_bin, raw), Some(Ok(_))))
    }

    /// Lie le circuit a un contact resolu (creation de l'etat si
    /// premier contact — consentement = etape 38).
    fn bind_contact(&self, cid: u32, pk_bin: &[u8]) {
        if let Some(b) = self.circuits.lock().unwrap().get_mut(&cid) {
            b.contact = Some(pk_bin.to_vec());
        }
        let mut contacts = self.contacts.lock().unwrap();
        if let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) {
            contacts
                .entry(pk_bin.to_vec())
                .or_insert_with(|| Contact {
                    pk,
                    recv_window: RecvWindow::new(&self.cfg),
                    send_seq: 0,
                    circuit: None,
                    greeted: false,
                })
                .circuit = Some(cid);
        }
    }

    /// Presence : maintient des points d'introduction sur notre
    /// swarm et re-publie l'annonce DHT a `announce_interval`
    /// (pendant de `ensure_introduction_points`+`reannounce` du
    /// moniteur de swarms BitTorrent).
    fn spawn_presence_monitor(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops.lock().unwrap().push(tx);
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(svc.cfg.announce_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = rx_stop.changed() => break,
                    _ = tick.tick() => {}
                }
                crate::ipv8_stack::ensure_introduction_points(&svc.tunnel, svc.own_mh);
                let t = svc.tunnel.clone();
                let mh = svc.own_mh;
                tokio::spawn(async move {
                    t.reannounce_intro_points(mh).await;
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_ipv8::endpoint::UdpEndpoint;
    use onionbit_ipv8::peer::Network;
    use onionbit_tunnel::settings::TunnelSettings;
    use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

    /// Service de test sur un tunnel loopback reel (le protocole ne
    /// depend pas du transport : les liaisons sont injectees a la
    /// main et `handle_incoming` est pilote directement).
    async fn make_service(hops: usize) -> (Arc<MessagingService>, LibNaClSecretKey) {
        let key = LibNaClSecretKey::generate();
        let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_run = ep.clone();
        tokio::spawn(async move {
            let _ = ep_run.run().await;
        });
        let tunnel = TunnelCommunity::new_with_id(
            key.clone(),
            Arc::new(Network::default()),
            ep,
            TunnelSettings {
                // Pas de points d'introduction automatiques en test.
                max_intro_points: 0,
                ..TunnelSettings::default()
            },
            TUNNEL_COMMUNITY_ID,
        )
        .await;
        let cfg = MessagingConfig::default();
        (MessagingService::start(tunnel, key.clone(), cfg, hops), key)
    }

    /// Injecte une liaison + un contact connus et retourne les cles.
    fn bind(
        svc: &MessagingService,
        cid: u32,
        peer: &LibNaClSecretKey,
        keys: &MessagingKeys,
    ) -> Vec<u8> {
        let pk_bin = peer.public_key().to_bin();
        svc.circuits.lock().unwrap().insert(
            cid,
            CircuitBinding {
                keys: keys.clone(),
                contact: Some(pk_bin.clone()),
            },
        );
        svc.contacts.lock().unwrap().insert(
            pk_bin.clone(),
            Contact {
                pk: peer.public_key(),
                recv_window: RecvWindow::new(&svc.cfg),
                send_seq: 0,
                circuit: Some(cid),
                greeted: true,
            },
        );
        pk_bin
    }

    fn wire(peer: &LibNaClSecretKey, seq: u64, key: &[u8; 32], body: &[u8]) -> Vec<u8> {
        Frame::new(MsgKind::Msg, seq, 1, body.to_vec())
            .seal(peer, key, &MessagingConfig::default())
            .unwrap()
    }

    /// Re-emission honnete a travers une reouverture de circuit
    /// (MS-2) : la trame livree sur le premier circuit est absorbee
    /// par la dedup quand elle revient sur le circuit ouvert en
    /// remplacement — la fenetre de reception vit sur le contact,
    /// pas sur le circuit.
    #[tokio::test]
    async fn dedup_survive_a_la_reouverture_du_circuit() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [7u8; 32],
            recv: [8u8; 32],
        };
        let pk_bin = bind(&svc, 11, &peer, &keys);
        let mut events = svc.subscribe();

        let w = wire(&peer, 0, &keys.recv, b"perdu en vol");
        svc.handle_incoming(11, &keys, &w);
        assert!(events.try_recv().is_ok(), "premiere livraison -> evenement");

        // Reouverture : un nouveau circuit se lie au meme contact,
        // l'ancien est oublie.
        svc.circuits.lock().unwrap().insert(
            12,
            CircuitBinding {
                keys: keys.clone(),
                contact: Some(pk_bin.clone()),
            },
        );
        svc.contacts
            .lock()
            .unwrap()
            .get_mut(&pk_bin)
            .expect("contact")
            .circuit = Some(12);
        svc.circuits.lock().unwrap().remove(&11);

        // La meme trame, relivree sur le circuit de remplacement :
        // absorbee par la dedup (fenetre attachee au contact).
        svc.handle_incoming(12, &keys, &w);
        assert!(
            events.try_recv().is_err(),
            "re-emission sur nouveau circuit -> aucun evenement"
        );
    }

    /// MS-9 — les trames messagerie ne ressemblent jamais a du uTP :
    /// meme si une trame s'egarait vers la lane BitTorrent, le filtre
    /// `could_be_utp` de `spawn_e2e_listener` l'ecarterait avant
    /// l'injection `TunnelUdpSocket`. Verifie la forme wire de chaque
    /// type de trame.
    #[tokio::test]
    async fn trames_jamais_acceptees_par_le_filtre_utp() {
        let peer = LibNaClSecretKey::generate();
        let key = [9u8; 32];
        let cfg = MessagingConfig::default();
        for kind in [MsgKind::Hello, MsgKind::Msg, MsgKind::Ack] {
            let wire = Frame::new(kind, 0, 1, b"corps".to_vec())
                .seal(&peer, &key, &cfg)
                .expect("seal");
            assert!(
                !onionbit_network_policy::exit_policy::could_be_utp(&wire),
                "trame {kind:?} acceptee comme uTP"
            );
        }
        // Le filtre rejette aussi les octets bruts bencode.
        assert!(!onionbit_network_policy::exit_policy::could_be_utp(
            b"d1:v i1ee"
        ));
    }

    /// Deduplication : la meme trame relivree (re-emission honnete,
    /// MS-2) ne produit qu'un seul evenement ; un `seq` rejoue sous
    /// un `id` neuf est rejete par la fenetre.
    #[tokio::test]
    async fn dedup_et_rejeu_au_niveau_service() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        bind(&svc, 42, &peer, &keys);
        let mut events = svc.subscribe();

        // La meme trame wire deux fois -> un seul evenement.
        let w = wire(&peer, 0, &keys.recv, b"premier");
        svc.handle_incoming(42, &keys, &w);
        svc.handle_incoming(42, &keys, &w);
        let ev = events.try_recv().expect("un evenement attendu");
        assert!(matches!(ev, MessagingEvent::Frame { .. }));
        assert!(
            events.try_recv().is_err(),
            "dedup : pas de second evenement"
        );

        // `seq` rejoue sous un `id` neuf -> rejete par la fenetre.
        let w2 = wire(&peer, 0, &keys.recv, b"rejeu");
        svc.handle_incoming(42, &keys, &w2);
        assert!(events.try_recv().is_err(), "seq rejoue -> aucun evenement");

        // `seq` suivant valide -> admis.
        let w3 = wire(&peer, 1, &keys.recv, b"second");
        svc.handle_incoming(42, &keys, &w3);
        assert!(events.try_recv().is_ok(), "seq suivant -> evenement");
    }

    /// Un `hello` sur un circuit non lie identifie l'emetteur :
    /// corps = `pk_bin`, signature verifiee contre elle, puis le
    /// circuit se lie a ce contact.
    #[tokio::test]
    async fn hello_identifie_l_emetteur_sur_circuit_non_lie() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [3u8; 32],
            recv: [4u8; 32],
        };
        svc.circuits.lock().unwrap().insert(
            7,
            CircuitBinding {
                keys: keys.clone(),
                contact: None,
            },
        );
        let hello = Frame::new(MsgKind::Hello, 0, 1, peer.public_key().to_bin())
            .seal(&peer, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(7, &keys, &hello);

        // Le circuit s'est lie a la cle du peer.
        let bound = svc
            .bound_contacts()
            .into_iter()
            .find(|(_, c)| *c == Some(7));
        assert!(bound.is_some(), "hello verifie -> contact lie au circuit");

        // Un `hello` signe par une AUTRE cle que celle declaree est
        // rejete (usurpation du corps).
        let liar = LibNaClSecretKey::generate();
        svc.circuits.lock().unwrap().insert(
            9,
            CircuitBinding {
                keys: keys.clone(),
                contact: None,
            },
        );
        let forged = Frame::new(MsgKind::Hello, 0, 1, peer.public_key().to_bin())
            .seal(&liar, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(9, &keys, &forged);
        assert!(
            svc.bound_contacts().iter().all(|(_, c)| *c != Some(9)),
            "hello usurpe -> pas de liaison"
        );
    }

    /// Les trames malformees ou non verifiables sont ecartees sans
    /// evenement ni panic (surface hostile — MS-3).
    #[tokio::test]
    async fn trames_hostiles_ecartees_sans_panic() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [5u8; 32],
            recv: [6u8; 32],
        };
        bind(&svc, 5, &peer, &keys);
        let mut events = svc.subscribe();
        for data in [
            &b""[..],
            b"pas du bencode",
            &[0u8; 64][..],
            b"d1:v i1ee",
            &vec![0xFF; 40_000][..],
        ] {
            svc.handle_incoming(5, &keys, data);
        }
        assert!(events.try_recv().is_err());
    }
}
