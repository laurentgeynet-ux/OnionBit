// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Payloads des cellules de tunnel (port de
//! `messaging/anonymization/payload.py`).
//!
//! Chaque payload "cellable" commence par `I circuit_id` (apres le
//! `msg_id` interne). Les formats `varlenH`, `20s`, `32s`,
//! `ip_address` suivent `serialization.py`.

use onionbit_ipv8::serializer::{Reader, Writer};
use onionbit_ipv8::Ipv8Error;
use onionbit_ipv8::UdpAddress;

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
    /// `TestRequestPayload` filaire d'`ipv8-rust-tunnels`
    /// (`socket.rs` — `identifier` u32, cellule 21). C'est le format
    /// effectivement emis par Tribler 8.x ; 19/20 (u16) est le format
    /// du backend Python pur de pyipv8.
    pub const SPEED_TEST_REQUEST: u8 = 21;
    /// `TestResponsePayload` filaire d'`ipv8-rust-tunnels` (cellule
    /// 22, `identifier` u32).
    pub const SPEED_TEST_RESPONSE: u8 = 22;
    /// `HTTPRequestPayload` (Tribler `core/tunnel/payload.py`,
    /// `ipv8-rust-tunnels`) : requete HTTP via un circuit (CONNECT
    /// SOCKS5, annonces tracker).
    pub const HTTP_REQUEST: u8 = 28;
    /// `HTTPResponsePayload` : reponse decoupee en chunks.
    pub const HTTP_RESPONSE: u8 = 29;
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

/// `PongPayload` (msg 7) : `I, H` — meme trame que `PingPayload`
/// mais `msg_id` distinct : un alias de type re-emettrait la reponse
/// comme un `ping`, entretenant une tempete ping/pong sans fin entre
/// les extremites (mesure mesh : ~6 000 cellules/s symetriques).
#[derive(Debug)]
pub struct TunnelPong {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier` (repercute depuis le ping).
    pub identifier: u16,
}

impl Cellable for TunnelPong {
    const MSG_ID: u8 = msg::PONG;
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

    /// Serialisation `serializer.pack("payload", rp_info)` de pyipv8
    /// (`NestedPayload`) : `>H` taille + corps. C'est la forme attendue
    /// dans `created-e2e.rp_info_enc` (`hidden_services.py:494`).
    pub fn pack_framed(&self) -> Result<Vec<u8>, Ipv8Error> {
        let mut inner = Writer::new();
        self.pack(&mut inner)?;
        let bytes = inner.into_bytes();
        let mut w = Writer::new();
        w.u16(bytes.len() as u16);
        w.bytes(&bytes);
        Ok(w.into_bytes())
    }

    /// Parse de la forme `NestedPayload` (`>H` taille + corps).
    pub fn unpack_framed(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let size = r.u16()? as usize;
        Self::unpack(&mut Reader::new(r.take(size)?))
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
            let mut inner = Writer::new();
            inner.ip_address(&p.address)?;
            inner.varlen_h(&p.key);
            inner.varlen_h(&p.seeder_pk);
            inner.u8(p.source);
            let bytes = inner.into_bytes();
            w.u16(bytes.len() as u16);
            w.bytes(&bytes);
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
            let size = r.u16()? as usize;
            let mut inner = Reader::new(r.take(size)?);
            peers.push(IntroductionInfo {
                address: inner.ip_address()?,
                key: inner.varlen_h()?.to_vec(),
                seeder_pk: inner.varlen_h()?.to_vec(),
                source: inner.u8()?,
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

/// `TestRequestPayload` filaire `ipv8-rust-tunnels` (cellule 21) :
/// `I, I, H, raw` — `identifier` **u32** (a la difference du
/// `TestRequest` Python u16 de la cellule 19).
#[derive(Debug)]
pub struct SpeedTestRequest {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier` de la requete de test (u32).
    pub identifier: u32,
    /// Taille de la reponse demandee (`response_size`).
    pub response_size: u16,
    /// `data` aleatoire de `request_size` octets.
    pub data: Vec<u8>,
}

impl Cellable for SpeedTestRequest {
    const MSG_ID: u8 = msg::SPEED_TEST_REQUEST;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u32(self.identifier);
        w.u16(self.response_size);
        w.raw(&self.data);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u32()?,
            response_size: r.u16()?,
            data: r.raw().to_vec(),
        })
    }
}

/// `TestResponsePayload` filaire `ipv8-rust-tunnels` (cellule 22) :
/// `I, I, raw` — `identifier` u32.
#[derive(Debug)]
pub struct SpeedTestResponse {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier` de la requete.
    pub identifier: u32,
    /// `data` de `response_size` octets.
    pub data: Vec<u8>,
}

impl Cellable for SpeedTestResponse {
    const MSG_ID: u8 = msg::SPEED_TEST_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u32(self.identifier);
        w.raw(&self.data);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u32()?,
            data: r.raw().to_vec(),
        })
    }
}

/// `HTTPRequestPayload` (msg 28) : `I, I, address, varlenH`
/// (`tribler/core/tunnel/payload.py`, `ipv8-rust-tunnels`).
#[derive(Debug)]
pub struct HttpRequest {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier` (u32 — cache de requetes).
    pub identifier: u32,
    /// `target` : destination HTTP.
    pub target: UdpAddress,
    /// `request` : octets de la requete HTTP brute.
    pub request: Vec<u8>,
}

impl Cellable for HttpRequest {
    const MSG_ID: u8 = msg::HTTP_REQUEST;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u32(self.identifier);
        w.ip_address(&self.target)?;
        w.varlen_h(&self.request);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u32()?,
            target: r.ip_address()?,
            request: r.varlen_h()?.to_vec(),
        })
    }
}

/// `HTTPResponsePayload` (msg 29) : `I, I, H, H, varlenH` — la
/// reponse TCP est decoupee en `total` chunks de 1400 octets max.
#[derive(Debug)]
pub struct HttpResponse {
    /// `circuit_id`.
    pub circuit_id: u32,
    /// `identifier`.
    pub identifier: u32,
    /// `part` : indice du chunk.
    pub part: u16,
    /// `total` : nombre total de chunks.
    pub total: u16,
    /// `response` : fragment.
    pub response: Vec<u8>,
}

impl Cellable for HttpResponse {
    const MSG_ID: u8 = msg::HTTP_RESPONSE;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.circuit_id);
        w.u32(self.identifier);
        w.u16(self.part);
        w.u16(self.total);
        w.varlen_h(&self.response);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            circuit_id: r.u32()?,
            identifier: r.u32()?,
            part: r.u16()?,
            total: r.u16()?,
            response: r.varlen_h()?.to_vec(),
        })
    }
}

/// Invariant de protocole verifie **a la compilation** : chaque
/// type `Cellable` porte un `MSG_ID` distinct. Une collision
/// silencieuse (alias de type, copier-coller d'impl) transformerait
/// une reponse en requete et entretiendrait des boucles de
/// protocole — c'est exactement la tempete ping/pong ~6 000
/// cellules/s causee par `type TunnelPong = TunnelPing`.
/// Toute collision refuse la compilation.
const _: () = {
    const IDS: &[u8] = &[
        Data::MSG_ID,
        Create::MSG_ID,
        Created::MSG_ID,
        Extend::MSG_ID,
        Extended::MSG_ID,
        TunnelPing::MSG_ID,
        TunnelPong::MSG_ID,
        EstablishIntro::MSG_ID,
        IntroEstablished::MSG_ID,
        EstablishRendezvous::MSG_ID,
        RendezvousEstablished::MSG_ID,
        LinkE2E::MSG_ID,
        LinkedE2E::MSG_ID,
        PeersRequest::MSG_ID,
        PeersResponse::MSG_ID,
        TestRequest::MSG_ID,
        TestResponse::MSG_ID,
        SpeedTestRequest::MSG_ID,
        SpeedTestResponse::MSG_ID,
        HttpRequest::MSG_ID,
        HttpResponse::MSG_ID,
    ];
    let mut i = 0;
    while i < IDS.len() {
        let mut j = i + 1;
        while j < IDS.len() {
            assert!(IDS[i] != IDS[j], "deux types Cellable partagent un MSG_ID");
            j += 1;
        }
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use onionbit_ipv8::serializer::{Reader, Writer};

    /// Le `msg_id` observe **sur le fil** pour un pong doit etre 7.
    /// On reproduit le chemin de `send_cell` (`pack` du corps puis
    /// `Cell::to_wire` avec `P::MSG_ID`) et on relit l'octet via
    /// `Cell::parse` — c'est ce qui aurait detecte l'alias
    /// `TunnelPong = TunnelPing` (le pong partait en `PING`).
    #[test]
    fn pong_porte_msg_id_7_sur_le_fil() {
        let p = TunnelPong {
            circuit_id: 0xAABBCCDD,
            identifier: 0x1234,
        };
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        let body = w.into_bytes();
        let wire = Cell::to_wire(
            &[0xAA; 22],
            p.circuit_id,
            TunnelPong::MSG_ID,
            &body[4..],
            false,
            false,
        );
        let cell = Cell::parse(&wire).unwrap();
        assert_eq!(cell.inner_msg_id, msg::PONG);
        assert_eq!(cell.circuit_id, p.circuit_id);
        assert_eq!(TunnelPing::MSG_ID, msg::PING);
        assert_ne!(TunnelPing::MSG_ID, TunnelPong::MSG_ID);
    }

    /// Roundtrip cellule 19 (`I, H, H, raw`) — format pyipv8 pur.
    #[test]
    fn test_request_u16_roundtrip() {
        let p = TestRequest {
            circuit_id: 0xAABBCCDD,
            identifier: 0x1234,
            response_size: 64,
            data: vec![1, 2, 3],
        };
        assert_eq!(TestRequest::MSG_ID, 19);
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        // I + H + H + raw(3) = 11 octets.
        let buf = w.into_bytes();
        assert_eq!(buf.len(), 4 + 2 + 2 + 3);
        let q = TestRequest::unpack(&mut Reader::new(&buf)).unwrap();
        assert_eq!(q.circuit_id, 0xAABBCCDD);
        assert_eq!(q.identifier, 0x1234);
        assert_eq!(q.response_size, 64);
        assert_eq!(q.data, vec![1, 2, 3]);
    }

    /// Roundtrip cellule 20 (`I, H, raw`).
    #[test]
    fn test_response_u16_roundtrip() {
        let p = TestResponse {
            circuit_id: 7,
            identifier: 0xBEEF,
            data: vec![9; 5],
        };
        assert_eq!(TestResponse::MSG_ID, 20);
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        let q = TestResponse::unpack(&mut Reader::new(&w.into_bytes())).unwrap();
        assert_eq!(q.circuit_id, 7);
        assert_eq!(q.identifier, 0xBEEF);
        assert_eq!(q.data.len(), 5);
    }

    /// Roundtrip cellule 21 (`I, I, H, raw`) — format
    /// `ipv8-rust-tunnels` avec `identifier` u32.
    #[test]
    fn speedtest_request_u32_roundtrip() {
        let p = SpeedTestRequest {
            circuit_id: 1,
            identifier: 0xDEADBEEF,
            response_size: 1024,
            data: vec![0xAA; 50],
        };
        assert_eq!(SpeedTestRequest::MSG_ID, 21);
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        let buf = w.into_bytes();
        // I + I + H + raw(50) = 60 octets ; l'identifier tient sur 4
        // octets (impossible avec le format 19 u16).
        assert_eq!(buf.len(), 4 + 4 + 2 + 50);
        let q = SpeedTestRequest::unpack(&mut Reader::new(&buf)).unwrap();
        assert_eq!(q.identifier, 0xDEADBEEF);
        assert_eq!(q.response_size, 1024);
        assert_eq!(q.data, vec![0xAA; 50]);
    }

    /// `PeersResponse` : `payload-list` pyipv8 = `B` count puis, par
    /// element, `>H` taille + `IntroductionInfo` serialise
    /// (`ListOf(NestedPayload)`). Regression : sans le prefixe de
    /// longueur, pyipv8 deserialise `IntroductionInfo` au mauvais offset
    /// (`Cannot unpack address type 0` chez Tribler 8.4.3).
    #[test]
    fn peers_response_nested_payload_wire() {
        let addr = UdpAddress::Ipv4("127.0.0.1:17789".parse::<std::net::SocketAddrV4>().unwrap());
        let p = PeersResponse {
            circuit_id: 42,
            identifier: 0x1234,
            info_hash: [0xAB; 20],
            peers: vec![
                IntroductionInfo {
                    address: addr.clone(),
                    key: vec![0x11; 66],
                    seeder_pk: vec![0x22; 74],
                    source: 1,
                },
                IntroductionInfo {
                    address: addr,
                    key: vec![0x33; 66],
                    seeder_pk: vec![0x44; 74],
                    source: 2,
                },
            ],
        };
        assert_eq!(PeersResponse::MSG_ID, 18);
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        let buf = w.into_bytes();

        // I(4) + H(2) + 20s(20) + B(1) = 27 octets avant la liste.
        assert_eq!(buf[26], 2);
        // Chaque element : `>H` taille + IntroductionInfo
        // (ip_address 7 + varlenH 2+66 + varlenH 2+74 + B 1 = 152).
        let elem_size = 7 + 2 + 66 + 2 + 74 + 1;
        let mut off = 27;
        for _ in 0..2 {
            let size = u16::from_be_bytes([buf[off], buf[off + 1]]) as usize;
            assert_eq!(size, elem_size);
            assert_eq!(buf[off + 2], 0x01); // type IPv4 du `ip_address`
            off += 2 + size;
        }
        assert_eq!(off, buf.len());

        let q = PeersResponse::unpack(&mut Reader::new(&buf)).unwrap();
        assert_eq!(q.peers.len(), 2);
        assert_eq!(q.peers[0].key, vec![0x11; 66]);
        assert_eq!(q.peers[1].source, 2);
        match &q.peers[0].address {
            UdpAddress::Ipv4(sa) => {
                assert_eq!(sa.ip().octets(), [127, 0, 0, 1]);
                assert_eq!(sa.port(), 17789);
            }
            other => panic!("adresse inattendue : {other:?}"),
        }
    }

    /// Roundtrip cellule 22 (`I, I, raw`).
    #[test]
    fn speedtest_response_u32_roundtrip() {
        let p = SpeedTestResponse {
            circuit_id: 3,
            identifier: 0xFFFFFFFE,
            data: vec![7; 2000],
        };
        assert_eq!(SpeedTestResponse::MSG_ID, 22);
        let mut w = Writer::new();
        p.pack(&mut w).unwrap();
        let q = SpeedTestResponse::unpack(&mut Reader::new(&w.into_bytes())).unwrap();
        assert_eq!(q.circuit_id, 3);
        assert_eq!(q.identifier, 0xFFFFFFFE);
        assert_eq!(q.data.len(), 2000);
    }
}
