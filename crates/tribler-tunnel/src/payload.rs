//! Payloads des cellules de tunnel (port de
//! `messaging/anonymization/payload.py`).
//!
//! Chaque payload "cellable" commence par `I circuit_id` (apres le
//! `msg_id` interne). Les formats `varlenH`, `20s`, `32s`,
//! `ip_address` suivent `serialization.py`.

use tribler_ipv8::serializer::{Reader, Writer};
use tribler_ipv8::Ipv8Error;
use tribler_ipv8::UdpAddress;

/// `msg_id` internes des messages de tunnel (`payload.py`).
pub mod msg {
    /// `CellPayload` — donnee brute (jamais emis comme message interne).
    pub const CELL: u8 = 0;
    /// `DataPayload`.
    pub const DATA: u8 = 1;
    /// `CreatePayload`.
    pub const CREATE: u8 = 2;
    /// `CreatedPayload`.
    pub const CREATED: u8 = 3;
    /// `ExtendPayload`.
    pub const EXTEND: u8 = 4;
    /// `ExtendedPayload`.
    pub const EXTENDED: u8 = 5;
    /// `PingPayload`.
    pub const PING: u8 = 6;
    /// `PongPayload`.
    pub const PONG: u8 = 7;
    /// `DestroyPayload` — envoye **hors cellule**, signe.
    pub const DESTROY: u8 = 8;
    /// `EstablishIntroPayload`.
    pub const ESTABLISH_INTRO: u8 = 9;
    /// `IntroEstablishedPayload`.
    pub const INTRO_ESTABLISHED: u8 = 10;
    /// `EstablishRendezvousPayload`.
    pub const ESTABLISH_RENDEZVOUS: u8 = 11;
    /// `RendezvousEstablishedPayload`.
    pub const RENDEZVOUS_ESTABLISHED: u8 = 12;
    /// `CreateE2EPayload` — paquet signe hors cellule.
    pub const CREATE_E2E: u8 = 13;
    /// `CreatedE2EPayload` — paquet signe hors cellule.
    pub const CREATED_E2E: u8 = 14;
    /// `LinkE2EPayload`.
    pub const LINK_E2E: u8 = 15;
    /// `LinkedE2EPayload`.
    pub const LINKED_E2E: u8 = 16;
    /// `PeersRequestPayload`.
    pub const PEERS_REQUEST: u8 = 17;
    /// `PeersResponsePayload`.
    pub const PEERS_RESPONSE: u8 = 18;
    /// `TestRequestPayload`.
    pub const TEST_REQUEST: u8 = 19;
    /// `TestResponsePayload`.
    pub const TEST_RESPONSE: u8 = 20;
}

/// Trait minimal de payload de cellule.
pub trait Cellable {
    /// `msg_id` interne.
    const MSG_ID: u8;
    /// `pack` du corps (apres msg_id).
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error>;
    /// `unpack` du corps (apres msg_id).
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error>
    where
        Self: Sized;
}

/// `DataPayload` (msg 1) : `I, address, address, raw`.
#[derive(Debug)]
pub struct Data {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// Destination finale (`dest_address`).
    pub dest_address: UdpAddress,
    /// Origine (`org_address`).
    pub org_address: UdpAddress,
    /// Donnees brutes.
    pub data: Vec<u8>,
}

impl Cellable for Data {
    const MSG_ID: u8 = msg::DATA;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.ip_address(&self.dest_address)?;
        w.ip_address(&self.org_address)?;
        w.raw(&self.data);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            dest_address: r.ip_address()?,
            org_address: r.ip_address()?,
            data: r.raw().to_vec(),
        })
    }
}

/// `CreatePayload` (msg 2) : `I, H, varlenH, varlenH` — envoye en clair.
#[derive(Debug)]
pub struct Create {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// Cle publique du noeud precedent (`node_public_key`,
    /// `key_to_bin`).
    pub node_public_key: Vec<u8>,
    /// Cle publique DH ephemere (`key` = `crypt_pk`).
    pub key: Vec<u8>,
}

impl Cellable for Create {
    const MSG_ID: u8 = msg::CREATE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.varlen_h(&self.node_public_key);
        w.varlen_h(&self.key);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
            node_public_key: r.varlen_h()?.to_vec(),
            key: r.varlen_h()?.to_vec(),
        })
    }
}

/// `CreatedPayload` (msg 3) : `I, H, varlenH, 32s, raw` — en clair.
#[derive(Debug)]
pub struct Created {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// Cle DH ephemere du repondant (`key` = `crypt_pk`).
    pub key: Vec<u8>,
    /// `auth` = `crypto_auth(shared_secret[:32], key)` (32 octets).
    pub auth: [u8; 32],
    /// Liste de candidats chiffree avec les cles de session
    /// (`candidates_enc`).
    pub candidates_enc: Vec<u8>,
}

impl Cellable for Created {
    const MSG_ID: u8 = msg::CREATED;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.varlen_h(&self.key);
        w.bytes(&self.auth);
        w.raw(&self.candidates_enc);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let key = r.varlen_h()?.to_vec();
        let mut auth = [0u8; 32];
        auth.copy_from_slice(r.take(32)?);
        Ok(Self {
            circuit_id,
            identifier,
            key,
            auth,
            candidates_enc: r.raw().to_vec(),
        })
    }
}

/// `ExtendPayload` (msg 4) : `I, H, varlenH, varlenH, ip_address`.
#[derive(Debug)]
pub struct Extend {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// Cle publique du prochain saut (`node_public_key`).
    pub node_public_key: Vec<u8>,
    /// Cle DH ephemere (`key`).
    pub key: Vec<u8>,
    /// Adresse du prochain saut (`node_addr`, `0.0.0.0:0` si inconnu —
    /// resolution via `request.candidates`/DHT).
    pub node_addr: UdpAddress,
}

impl Cellable for Extend {
    const MSG_ID: u8 = msg::EXTEND;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.varlen_h(&self.node_public_key);
        w.varlen_h(&self.key);
        w.ip_address(&self.node_addr)?;
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
            node_public_key: r.varlen_h()?.to_vec(),
            key: r.varlen_h()?.to_vec(),
            node_addr: r.ip_address()?,
        })
    }
}

/// `ExtendedPayload` (msg 5) : `I, H, varlenH, 32s, raw`.
#[derive(Debug)]
pub struct Extended {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// Cle DH du nouveau saut.
    pub key: Vec<u8>,
    /// `auth`.
    pub auth: [u8; 32],
    /// Candidats chiffres.
    pub candidates_enc: Vec<u8>,
}

impl Cellable for Extended {
    const MSG_ID: u8 = msg::EXTENDED;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.varlen_h(&self.key);
        w.bytes(&self.auth);
        w.raw(&self.candidates_enc);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let key = r.varlen_h()?.to_vec();
        let mut auth = [0u8; 32];
        auth.copy_from_slice(r.take(32)?);
        Ok(Self {
            circuit_id,
            identifier,
            key,
            auth,
            candidates_enc: r.raw().to_vec(),
        })
    }
}

/// `PingPayload` (msg 6) : `I, H`.
#[derive(Debug)]
pub struct TunnelPing {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
}

impl Cellable for TunnelPing {
    const MSG_ID: u8 = msg::PING;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
        })
    }
}

/// `PongPayload` (msg 7) : `I, H`.
pub type TunnelPong = TunnelPing;

/// `DestroyPayload` (msg 8, paquet **signe** hors cellule) : `I, H`.
#[derive(Debug)]
pub struct Destroy {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `reason`.
    pub reason: u16,
}

impl Destroy {
    /// Corps du paquet signe (apres global_time).
    pub fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.reason);
        Ok(())
    }
    /// Parse du corps.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            reason: r.u16()?,
        })
    }
}

/// `EstablishIntroPayload` (msg 9) : `I, H, 20s, varlenH`.
#[derive(Debug)]
pub struct EstablishIntro {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `info_hash`.
    pub info_hash: [u8; 20],
    /// `public_key` du seeder (cle du service cache).
    pub public_key: Vec<u8>,
}

impl Cellable for EstablishIntro {
    const MSG_ID: u8 = msg::ESTABLISH_INTRO;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.bytes(&self.info_hash);
        w.varlen_h(&self.public_key);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(r.take(20)?);
        Ok(Self {
            circuit_id,
            identifier,
            info_hash,
            public_key: r.varlen_h()?.to_vec(),
        })
    }
}

/// `IntroEstablishedPayload` (msg 10) : `I, H`.
#[derive(Debug)]
pub struct IntroEstablished {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
}

impl Cellable for IntroEstablished {
    const MSG_ID: u8 = msg::INTRO_ESTABLISHED;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
        })
    }
}

/// `EstablishRendezvousPayload` (msg 11) : `I, H, 20s`.
#[derive(Debug)]
pub struct EstablishRendezvous {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `cookie`.
    pub cookie: [u8; 20],
}

impl Cellable for EstablishRendezvous {
    const MSG_ID: u8 = msg::ESTABLISH_RENDEZVOUS;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.bytes(&self.cookie);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let mut cookie = [0u8; 20];
        cookie.copy_from_slice(r.take(20)?);
        Ok(Self {
            circuit_id,
            identifier,
            cookie,
        })
    }
}

/// `RendezvousEstablishedPayload` (msg 12) : `I, H, ip_address`.
#[derive(Debug)]
pub struct RendezvousEstablished {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `rendezvous_point` address.
    pub rendezvous_point: UdpAddress,
}

impl Cellable for RendezvousEstablished {
    const MSG_ID: u8 = msg::RENDEZVOUS_ESTABLISHED;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.ip_address(&self.rendezvous_point)?;
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
            rendezvous_point: r.ip_address()?,
        })
    }
}

/// `CreateE2EPayload` (msg 13, paquet **non signe** hors cellule :
/// `ezr_pack(..., sig=False)` dans `hidden_services`) :
/// `H, 20s, varlenH, varlenH`.
#[derive(Debug)]
pub struct CreateE2E {
    /// `identifier`.
    pub identifier: u16,
    /// `info_hash`.
    pub info_hash: [u8; 20],
    /// `node_public_key`.
    pub node_public_key: Vec<u8>,
    /// `key` DH ephemere.
    pub key: Vec<u8>,
}

impl CreateE2E {
    /// Corps du paquet (apres global_time).
    pub fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u16(self.identifier);
        w.bytes(&self.info_hash);
        w.varlen_h(&self.node_public_key);
        w.varlen_h(&self.key);
        Ok(())
    }
    /// Parse du corps.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let identifier = r.u16()?;
        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(r.take(20)?);
        Ok(Self {
            identifier,
            info_hash,
            node_public_key: r.varlen_h()?.to_vec(),
            key: r.varlen_h()?.to_vec(),
        })
    }
}

/// `CreatedE2EPayload` (msg 14, **non signe**) : `H, varlenH, 32s, raw`.
#[derive(Debug)]
pub struct CreatedE2E {
    /// `identifier`.
    pub identifier: u16,
    /// `key`.
    pub key: Vec<u8>,
    /// `auth`.
    pub auth: [u8; 32],
    /// `rp_info_enc`.
    pub rp_info_enc: Vec<u8>,
}

impl CreatedE2E {
    /// Corps du paquet.
    pub fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u16(self.identifier);
        w.varlen_h(&self.key);
        w.bytes(&self.auth);
        w.raw(&self.rp_info_enc);
        Ok(())
    }
    /// Parse du corps.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let identifier = r.u16()?;
        let key = r.varlen_h()?.to_vec();
        let mut auth = [0u8; 32];
        auth.copy_from_slice(r.take(32)?);
        Ok(Self {
            identifier,
            key,
            auth,
            rp_info_enc: r.raw().to_vec(),
        })
    }
}

/// `LinkE2EPayload` (msg 15) : `I, H, 20s`.
#[derive(Debug)]
pub struct LinkE2E {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `cookie`.
    pub cookie: [u8; 20],
}

impl Cellable for LinkE2E {
    const MSG_ID: u8 = msg::LINK_E2E;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.bytes(&self.cookie);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let mut cookie = [0u8; 20];
        cookie.copy_from_slice(r.take(20)?);
        Ok(Self {
            circuit_id,
            identifier,
            cookie,
        })
    }
}

/// `LinkedE2EPayload` (msg 16) : `I, H`.
#[derive(Debug)]
pub struct LinkedE2E {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
}

impl Cellable for LinkedE2E {
    const MSG_ID: u8 = msg::LINKED_E2E;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
        })
    }
}

/// `RendezvousInfo` (corps chiffre dans `CreatedE2E.rp_info_enc`) :
/// `ip_address, varlenH, 20s`.
#[derive(Debug, Clone)]
pub struct RendezvousInfo {
    /// Adresse du point de rendez-vous.
    pub address: UdpAddress,
    /// Cle publique du dernier saut du circuit RP (`key_to_bin`).
    pub key: Vec<u8>,
    /// Cookie de rendez-vous.
    pub cookie: [u8; 20],
}

impl RendezvousInfo {
    /// Corps serialise.
    pub fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.ip_address(&self.address)?;
        w.varlen_h(&self.key);
        w.bytes(&self.cookie);
        Ok(())
    }
    /// Parse du corps.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let address = r.ip_address()?;
        let key = r.varlen_h()?.to_vec();
        let mut cookie = [0u8; 20];
        cookie.copy_from_slice(r.take(20)?);
        Ok(Self {
            address,
            key,
            cookie,
        })
    }
}

/// `PeersRequestPayload` (msg 17) : `I, H, 20s`.
#[derive(Debug)]
pub struct PeersRequest {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `info_hash`.
    pub info_hash: [u8; 20],
}

impl Cellable for PeersRequest {
    const MSG_ID: u8 = msg::PEERS_REQUEST;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.bytes(&self.info_hash);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(r.take(20)?);
        Ok(Self {
            circuit_id,
            identifier,
            info_hash,
        })
    }
}

/// `IntroductionInfo` (element de `PeersResponse.peers`) :
/// `ip_address, varlenH, varlenH, B`.
#[derive(Debug, Clone)]
pub struct IntroductionInfo {
    /// Adresse du point d'introduction.
    pub address: UdpAddress,
    /// Cle publique du point d'introduction.
    pub key: Vec<u8>,
    /// Cle publique du seeder.
    pub seeder_pk: Vec<u8>,
    /// `source` (PEER_SOURCE_*).
    pub source: u8,
}

/// `PeersResponsePayload` (msg 18) : `I, H, 20s, [IntroductionInfo]`.
#[derive(Debug)]
pub struct PeersResponse {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `info_hash`.
    pub info_hash: [u8; 20],
    /// `peers` (liste `payload-list`, compteur `>B`).
    pub peers: Vec<IntroductionInfo>,
}

impl Cellable for PeersResponse {
    const MSG_ID: u8 = msg::PEERS_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.bytes(&self.info_hash);
        w.u8(self.peers.len() as u8);
        for p in &self.peers {
            w.ip_address(&p.address)?;
            w.varlen_h(&p.key);
            w.varlen_h(&p.seeder_pk);
            w.u8(p.source);
        }
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let circuit_id = r.u32()?;
        let identifier = r.u16()?;
        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(r.take(20)?);
        let count = r.u8()? as usize;
        let mut peers = Vec::with_capacity(count.min(255));
        for _ in 0..count {
            peers.push(IntroductionInfo {
                address: r.ip_address()?,
                key: r.varlen_h()?.to_vec(),
                seeder_pk: r.varlen_h()?.to_vec(),
                source: r.u8()?,
            });
        }
        Ok(Self {
            circuit_id,
            identifier,
            info_hash,
            peers,
        })
    }
}

/// `TestRequestPayload` (msg 19) : `I, H, H, raw`.
#[derive(Debug)]
pub struct TestRequest {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// Taille de la reponse demandee (`response_size`).
    pub response_size: u16,
    /// `data`.
    pub data: Vec<u8>,
}

impl Cellable for TestRequest {
    const MSG_ID: u8 = msg::TEST_REQUEST;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.u16(self.response_size);
        w.raw(&self.data);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
            response_size: r.u16()?,
            data: r.raw().to_vec(),
        })
    }
}

/// `TestResponsePayload` (msg 20) : `I, H, raw`.
#[derive(Debug)]
pub struct TestResponse {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u16,
    /// `data`.
    pub data: Vec<u8>,
}

impl Cellable for TestResponse {
    const MSG_ID: u8 = msg::TEST_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u16(self.identifier);
        w.raw(&self.data);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u16()?,
            data: r.raw().to_vec(),
        })
    }
}
