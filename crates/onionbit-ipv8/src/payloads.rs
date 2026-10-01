// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Payloads de decouverte de pairs (equivalent de
//! `peerdiscovery/payload.py` + `messaging/payload.py`).
//!
//! `msg_id` et `format_list` identiques au Python — la compatibilite
//! filaire est la regle n°1 (cf. AGENTS.md).

use crate::address::UdpAddress;
use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};

/// Identifiants de messages de la `DiscoveryCommunity`.
pub mod msg {
    /// `SimilarityRequestPayload`.
    pub const SIMILARITY_REQUEST: u8 = 1;
    /// `SimilarityResponsePayload`.
    pub const SIMILARITY_RESPONSE: u8 = 2;
    /// `PingPayload`.
    pub const PING: u8 = 3;
    /// `PongPayload`.
    pub const PONG: u8 = 4;
    /// `NewPuncturePayload`.
    pub const NEW_PUNCTURE: u8 = 231;
    /// `NewPunctureRequestPayload`.
    pub const NEW_PUNCTURE_REQUEST: u8 = 232;
    /// `NewIntroductionResponsePayload`.
    pub const NEW_INTRODUCTION_RESPONSE: u8 = 233;
    /// `NewIntroductionRequestPayload`.
    pub const NEW_INTRODUCTION_REQUEST: u8 = 234;
    /// `IntroductionResponsePayload` (ancien format, IPv4).
    pub const INTRODUCTION_RESPONSE: u8 = 245;
    /// `IntroductionRequestPayload` (ancien format, IPv4).
    pub const INTRODUCTION_REQUEST: u8 = 246;
    /// `PuncturePayload`.
    pub const PUNCTURE: u8 = 249;
    /// `PunctureRequestPayload`.
    pub const PUNCTURE_REQUEST: u8 = 250;
}

/// Type de connexion annonce (`encode_connection_type` Python).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    /// Inconnu (defaut Python `"unknown"`).
    Unknown,
    /// Adresse publique directe.
    Public,
    /// Derriere un NAT symetrique.
    SymmetricNat,
}

impl ConnectionType {
    /// `(bit_0, bit_1)` comme `encode_connection_type`.
    pub fn encode(self) -> (bool, bool) {
        match self {
            Self::Public => (true, false),
            Self::SymmetricNat => (true, true),
            Self::Unknown => (false, false),
        }
    }

    /// Decode `(bit_0, bit_1)` comme `decode_connection_type`.
    pub fn decode(b0: bool, b1: bool) -> Self {
        match (b0, b1) {
            (true, false) => Self::Public,
            (true, true) => Self::SymmetricNat,
            _ => Self::Unknown,
        }
    }
}

/// Un payload sait se serialiser et se deserialiser.
pub trait Payload: Sized {
    /// `msg_id` dans la community.
    const MSG_ID: u8;
    /// Serialise dans `w` (sans auth ni dist ni signature).
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error>;
    /// Deserialise depuis `r` (payload deja extrait du paquet).
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error>;
}

/// `SimilarityRequestPayload` (msg 1) :
/// `H, ipv4, ipv4, bits, raw`.
#[derive(Debug)]
pub struct SimilarityRequest {
    /// Identifiant de la requete (mod 65536).
    pub identifier: u16,
    /// Adresse LAN estimee.
    pub lan_address: UdpAddress,
    /// Adresse WAN estimee.
    pub wan_address: UdpAddress,
    /// Type de connexion.
    pub connection_type: ConnectionType,
    /// community_ids preferees (20 octets chacune).
    pub preference_list: Vec<[u8; 20]>,
}

impl Payload for SimilarityRequest {
    const MSG_ID: u8 = msg::SIMILARITY_REQUEST;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        let (b0, b1) = self.connection_type.encode();
        w.u16(self.identifier);
        w.ipv4(&self.lan_address)?;
        w.ipv4(&self.wan_address)?;
        w.bits([b0, b1, false, false, false, false, false, false]);
        let raw: Vec<u8> = self.preference_list.iter().flatten().copied().collect();
        w.raw(&raw);
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let identifier = r.u16()?;
        let lan_address = r.ipv4()?;
        let wan_address = r.ipv4()?;
        let bits = r.bits()?;
        let raw = r.raw();
        if !raw.len().is_multiple_of(20) {
            return Err(Ipv8Error::Malformed("preference_list non multiple de 20"));
        }
        Ok(Self {
            identifier,
            lan_address,
            wan_address,
            connection_type: ConnectionType::decode(bits[0], bits[1]),
            preference_list: raw.as_chunks::<20>().0.to_vec(),
        })
    }
}

/// `SimilarityResponsePayload` (msg 2) : `H, varlenHx20, raw`.
/// `tb_overlap` = liste de (service_id 20o, overlap u32).
#[derive(Debug)]
pub struct SimilarityResponse {
    /// Identifiant (miroir de la requete).
    pub identifier: u16,
    /// community_ids connues de l'emetteur.
    pub preference_list: Vec<[u8; 20]>,
    /// (service_id, score d'overlap).
    pub tb_overlap: Vec<([u8; 20], u32)>,
}

impl Payload for SimilarityResponse {
    const MSG_ID: u8 = msg::SIMILARITY_RESPONSE;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u16(self.identifier);
        w.varlen_h_x20(&self.preference_list);
        for (service, overlap) in &self.tb_overlap {
            w.bytes(service);
            w.u32(*overlap);
        }
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let identifier = r.u16()?;
        let preference_list = r.varlen_h_x20()?;
        let raw = r.raw();
        if !raw.len().is_multiple_of(24) {
            return Err(Ipv8Error::Malformed("tb_overlap non multiple de 24"));
        }
        let tb_overlap = raw
            .as_chunks::<24>()
            .0
            .iter()
            .map(|c| {
                (
                    <[u8; 20]>::try_from(&c[..20]).unwrap(),
                    u32::from_be_bytes(c[20..24].try_into().unwrap()),
                )
            })
            .collect();
        Ok(Self {
            identifier,
            preference_list,
            tb_overlap,
        })
    }
}

/// `PingPayload` (msg 3) : `H`.
#[derive(Debug)]
pub struct Ping {
    /// Nonce.
    pub identifier: u16,
}

impl Payload for Ping {
    const MSG_ID: u8 = msg::PING;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u16(self.identifier);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u16()?,
        })
    }
}

/// `PongPayload` (msg 4) : `H` (meme layout que `Ping`).
#[derive(Debug)]
pub struct Pong {
    /// Nonce en echo.
    pub identifier: u16,
}

impl Payload for Pong {
    const MSG_ID: u8 = msg::PONG;
    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u16(self.identifier);
        Ok(())
    }
    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u16()?,
        })
    }
}

/// `IntroductionRequestPayload` (msg 246, ancien format IPv4) :
/// `ipv4, ipv4, ipv4, bits, H, raw`.
#[derive(Debug)]
pub struct IntroductionRequest {
    /// Adresse du destinataire (WAN cense).
    pub destination_address: UdpAddress,
    /// LAN de l'emetteur.
    pub source_lan_address: UdpAddress,
    /// WAN de l'emetteur.
    pub source_wan_address: UdpAddress,
    /// Demande d'introduction a un autre pair.
    pub advice: bool,
    /// L'emetteur supporte le nouveau format `ip_address`.
    pub supports_new_style: bool,
    /// Type de connexion.
    pub connection_type: ConnectionType,
    /// Identifiant a renvoyer dans la reponse.
    pub identifier: u16,
    /// Octets libres piggybackes.
    pub extra_bytes: Vec<u8>,
}

impl Payload for IntroductionRequest {
    const MSG_ID: u8 = msg::INTRODUCTION_REQUEST;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        let (b0, b1) = self.connection_type.encode();
        w.ipv4(&self.destination_address)?;
        w.ipv4(&self.source_lan_address)?;
        w.ipv4(&self.source_wan_address)?;
        // bits : conn0, conn1, supports_new_style, 0, 0, 0, 0, advice
        w.bits([
            b0,
            b1,
            self.supports_new_style,
            false,
            false,
            false,
            false,
            self.advice,
        ]);
        w.u16(self.identifier);
        w.raw(&self.extra_bytes);
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let destination_address = r.ipv4()?;
        let source_lan_address = r.ipv4()?;
        let source_wan_address = r.ipv4()?;
        let bits = r.bits()?;
        let identifier = r.u16()?;
        let extra_bytes = r.raw().to_vec();
        Ok(Self {
            destination_address,
            source_lan_address,
            source_wan_address,
            advice: bits[7],
            supports_new_style: bits[2],
            connection_type: ConnectionType::decode(bits[0], bits[1]),
            identifier,
            extra_bytes,
        })
    }
}

/// `IntroductionResponsePayload` (msg 245) :
/// `ipv4 x5, bits, H, raw`.
#[derive(Debug)]
pub struct IntroductionResponse {
    /// Adresse du destinataire (= l'emetteur de la requete).
    pub destination_address: UdpAddress,
    /// LAN de l'emetteur de la reponse.
    pub source_lan_address: UdpAddress,
    /// WAN de l'emetteur de la reponse.
    pub source_wan_address: UdpAddress,
    /// LAN du pair introduit (`0.0.0.0:0` si aucun).
    pub lan_introduction_address: UdpAddress,
    /// WAN du pair introduit.
    pub wan_introduction_address: UdpAddress,
    /// Type de connexion de l'emetteur.
    pub connection_type: ConnectionType,
    /// L'emetteur supporte le nouveau format.
    pub supports_new_style: bool,
    /// Le pair introduit supporte le nouveau format.
    pub intro_supports_new_style: bool,
    /// Limite de pairs atteinte cote emetteur.
    pub peer_limit_reached: bool,
    /// Identifiant de la requete d'origine.
    pub identifier: u16,
    /// Octets libres.
    pub extra_bytes: Vec<u8>,
}

impl Payload for IntroductionResponse {
    const MSG_ID: u8 = msg::INTRODUCTION_RESPONSE;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        let (b0, b1) = self.connection_type.encode();
        w.ipv4(&self.destination_address)?;
        w.ipv4(&self.source_lan_address)?;
        w.ipv4(&self.source_wan_address)?;
        w.ipv4(&self.lan_introduction_address)?;
        w.ipv4(&self.wan_introduction_address)?;
        // bits : conn0, conn1, 0, supports_new_style,
        //        intro_supports_new_style, peer_limit_reached, 0, 0
        w.bits([
            b0,
            b1,
            false,
            self.supports_new_style,
            self.intro_supports_new_style,
            self.peer_limit_reached,
            false,
            false,
        ]);
        w.u16(self.identifier);
        w.raw(&self.extra_bytes);
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let destination_address = r.ipv4()?;
        let source_lan_address = r.ipv4()?;
        let source_wan_address = r.ipv4()?;
        let lan_introduction_address = r.ipv4()?;
        let wan_introduction_address = r.ipv4()?;
        let bits = r.bits()?;
        let identifier = r.u16()?;
        let extra_bytes = r.raw().to_vec();
        Ok(Self {
            destination_address,
            source_lan_address,
            source_wan_address,
            lan_introduction_address,
            wan_introduction_address,
            connection_type: ConnectionType::decode(bits[0], bits[1]),
            supports_new_style: bits[3],
            intro_supports_new_style: bits[4],
            peer_limit_reached: bits[5],
            identifier,
            extra_bytes,
        })
    }
}

/// `NewIntroductionRequestPayload` (msg 234) :
/// `ip_address x3, H, bits, raw`.
///
/// Bits (ordre `names` Python, MSB-first) : `connection_type_0`,
/// `connection_type_1`, `supports_new_style`, `dflag1`, `dflag2`,
/// `tunnel`, `sync`, `advice`.
#[derive(Debug)]
pub struct NewIntroductionRequest {
    /// Adresse de destination (WAN cense).
    pub destination_address: UdpAddress,
    /// LAN de l'emetteur.
    pub source_lan_address: UdpAddress,
    /// WAN de l'emetteur.
    pub source_wan_address: UdpAddress,
    /// Identifiant a renvoyer.
    pub identifier: u16,
    /// Type de connexion.
    pub connection_type: ConnectionType,
    /// `supports_new_style`.
    pub supports_new_style: bool,
    /// `tunnel` (le demandeur veut du tunnel).
    pub tunnel: bool,
    /// `sync` (le demandeur veut synchroniser).
    pub sync: bool,
    /// `advice` (introduction a un autre pair souhaitee).
    pub advice: bool,
    /// Octets libres.
    pub extra_bytes: Vec<u8>,
}

impl Payload for NewIntroductionRequest {
    const MSG_ID: u8 = msg::NEW_INTRODUCTION_REQUEST;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        let (b0, b1) = self.connection_type.encode();
        w.ip_address(&self.destination_address)?;
        w.ip_address(&self.source_lan_address)?;
        w.ip_address(&self.source_wan_address)?;
        w.u16(self.identifier);
        w.bits([
            b0,
            b1,
            self.supports_new_style,
            false,
            false,
            self.tunnel,
            self.sync,
            self.advice,
        ]);
        w.raw(&self.extra_bytes);
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        let destination_address = r.ip_address()?;
        let source_lan_address = r.ip_address()?;
        let source_wan_address = r.ip_address()?;
        let identifier = r.u16()?;
        let bits = r.bits()?;
        let extra_bytes = r.raw().to_vec();
        Ok(Self {
            destination_address,
            source_lan_address,
            source_wan_address,
            identifier,
            connection_type: ConnectionType::decode(bits[0], bits[1]),
            supports_new_style: bits[2],
            tunnel: bits[5],
            sync: bits[6],
            advice: bits[7],
            extra_bytes,
        })
    }
}

/// `NewIntroductionResponsePayload` (msg 233) :
/// `ip_address x5, H, bits, raw`.
///
/// Bits : `intro_supports_new_style` (bit 0), puis `flag1..flag7`.
#[derive(Debug)]
pub struct NewIntroductionResponse {
    /// Adresse du destinataire.
    pub destination_address: UdpAddress,
    /// LAN de l'emetteur.
    pub source_lan_address: UdpAddress,
    /// WAN de l'emetteur.
    pub source_wan_address: UdpAddress,
    /// LAN du pair introduit.
    pub lan_introduction_address: UdpAddress,
    /// WAN du pair introduit.
    pub wan_introduction_address: UdpAddress,
    /// Identifiant de la requete.
    pub identifier: u16,
    /// `intro_supports_new_style`.
    pub intro_supports_new_style: bool,
    /// Octets libres.
    pub extra_bytes: Vec<u8>,
}

impl Payload for NewIntroductionResponse {
    const MSG_ID: u8 = msg::NEW_INTRODUCTION_RESPONSE;

    fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.ip_address(&self.destination_address)?;
        w.ip_address(&self.source_lan_address)?;
        w.ip_address(&self.source_wan_address)?;
        w.ip_address(&self.lan_introduction_address)?;
        w.ip_address(&self.wan_introduction_address)?;
        w.u16(self.identifier);
        w.bits([
            self.intro_supports_new_style,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        ]);
        w.raw(&self.extra_bytes);
        Ok(())
    }

    fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            destination_address: r.ip_address()?,
            source_lan_address: r.ip_address()?,
            source_wan_address: r.ip_address()?,
            lan_introduction_address: r.ip_address()?,
            wan_introduction_address: r.ip_address()?,
            identifier: r.u16()?,
            intro_supports_new_style: {
                let bits = r.bits()?;
                bits[0]
            },
            extra_bytes: r.raw().to_vec(),
        })
    }
}

/// `PunctureRequestPayload` (msg 250, **non signe**) :
/// `ipv4, ipv4, H`.
#[derive(Debug)]
pub struct PunctureRequestPayload {
    /// Adresse LAN du pair a puncturer.
    pub lan_walker_address: UdpAddress,
    /// Adresse WAN du pair a puncturer.
    pub wan_walker_address: UdpAddress,
    /// Identifiant de la demande.
    pub identifier: u16,
}

impl PunctureRequestPayload {
    /// Deserialise.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            lan_walker_address: r.ipv4()?,
            wan_walker_address: r.ipv4()?,
            identifier: r.u16()?,
        })
    }
}

/// `NewPunctureRequestPayload` (msg 232, **non signe**) :
/// `ip_address, ip_address, H`.
#[derive(Debug)]
pub struct NewPunctureRequestPayload {
    /// LAN du pair a puncturer.
    pub lan_walker_address: UdpAddress,
    /// WAN du pair a puncturer.
    pub wan_walker_address: UdpAddress,
    /// Identifiant de la demande.
    pub identifier: u16,
}

impl NewPunctureRequestPayload {
    /// Deserialise.
    pub fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            lan_walker_address: r.ip_address()?,
            wan_walker_address: r.ip_address()?,
            identifier: r.u16()?,
        })
    }
}
