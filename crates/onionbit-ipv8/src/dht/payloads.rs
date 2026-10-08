// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Payloads DHT IPv8 (equivalent de `dht/payload.py`) : `msg_id` et
//! `format_list` identiques au Python, plus la serialisation des
//! valeurs stockees (`serialize_value`/`unserialize_value`).

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, SIGNATURE_LENGTH};

use crate::address::UdpAddress;
use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};

/// `msg_id` des messages DHT (`dht/payload.py`).
pub mod msg {
    /// `PingRequestPayload`.
    pub const PING_REQUEST: u8 = 1;
    /// `PingResponsePayload`.
    pub const PING_RESPONSE: u8 = 2;
    /// `StoreRequestPayload`.
    pub const STORE_REQUEST: u8 = 3;
    /// `StoreResponsePayload`.
    pub const STORE_RESPONSE: u8 = 4;
    /// `FindRequestPayload`.
    pub const FIND_REQUEST: u8 = 5;
    /// `FindResponsePayload`.
    pub const FIND_RESPONSE: u8 = 6;
    /// `StorePeerRequestPayload` (DHTDiscoveryCommunity).
    pub const STORE_PEER_REQUEST: u8 = 7;
    /// `StorePeerResponsePayload` (DHTDiscoveryCommunity).
    pub const STORE_PEER_RESPONSE: u8 = 8;
    /// `ConnectPeerRequestPayload` (DHTDiscoveryCommunity).
    pub const CONNECT_PEER_REQUEST: u8 = 9;
    /// `ConnectPeerResponsePayload` (DHTDiscoveryCommunity).
    pub const CONNECT_PEER_RESPONSE: u8 = 10;
}

/// `DHT_ENTRY_STR` : valeur non signee (`0x00 + StrPayload`).
pub const DHT_ENTRY_STR: u8 = 0;
/// `DHT_ENTRY_STR_SIGNED` : valeur signee (`0x01 + SignedStrPayload + sig`).
pub const DHT_ENTRY_STR_SIGNED: u8 = 1;

/// `MAX_ENTRY_SIZE` Python : taille max d'une valeur serialisee.
pub const MAX_ENTRY_SIZE: usize = 170;

/// Lit un `varlenH-list` (`B count` + items `varlenH`).
fn read_varlen_h_list(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>, Ipv8Error> {
    let count = r.u8()? as usize;
    let mut out = Vec::with_capacity(count.min(255));
    for _ in 0..count {
        out.push(r.varlen_h()?.to_vec());
    }
    Ok(out)
}

/// Ecrit un `varlenH-list`.
fn write_varlen_h_list(w: &mut Writer, items: &[Vec<u8>]) {
    w.u8(items.len() as u8);
    for it in items {
        w.varlen_h(it);
    }
}

/// Lit un `node-list` (`B count` + items `ip_address + varlenH`),
/// cf. `ListOf(NodePacker)`.
fn read_node_list(r: &mut Reader<'_>) -> Result<Vec<(UdpAddress, Vec<u8>)>, Ipv8Error> {
    let count = r.u8()? as usize;
    let mut out = Vec::with_capacity(count.min(255));
    for _ in 0..count {
        let addr = r.ip_address()?;
        let key = r.varlen_h()?.to_vec();
        out.push((addr, key));
    }
    Ok(out)
}

/// Ecrit un `node-list`.
fn write_node_list(w: &mut Writer, nodes: &[(UdpAddress, Vec<u8>)]) -> Result<(), Ipv8Error> {
    w.u8(nodes.len() as u8);
    for (addr, key) in nodes {
        w.ip_address(addr)?;
        w.varlen_h(key);
    }
    Ok(())
}

/// `PingRequestPayload` / `PingResponsePayload` : `I`.
#[derive(Debug)]
pub struct PingRequest {
    /// Nonce de la requete (`RandomNumberCache.number`).
    pub identifier: u32,
}

impl PingRequest {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
        })
    }
}

/// `PingResponsePayload` : `I`.
#[derive(Debug)]
pub struct PingResponse {
    /// Nonce en echo.
    pub identifier: u32,
}

impl PingResponse {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
        })
    }
}

/// `StoreRequestPayload` : `I, 20s token, 20s target, varlenH-list`.
#[derive(Debug)]
pub struct StoreRequest {
    /// Nonce.
    pub identifier: u32,
    /// Jeton anti-spoofing (`generate_token`).
    pub token: [u8; 20],
    /// Cle cible.
    pub target: [u8; 20],
    /// Valeurs serialisees (`serialize_value`).
    pub values: Vec<Vec<u8>>,
}

impl StoreRequest {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
        w.bytes(&self.token);
        w.bytes(&self.target);
        write_varlen_h_list(w, &self.values);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            token: r.take(20)?.try_into().unwrap(),
            target: r.take(20)?.try_into().unwrap(),
            values: read_varlen_h_list(r)?,
        })
    }
}

/// `StoreResponsePayload` : `I`.
#[derive(Debug)]
pub struct StoreResponse {
    /// Nonce en echo.
    pub identifier: u32,
}

impl StoreResponse {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
        })
    }
}

/// `FindRequestPayload` : `I, ip_address, 20s, I, ?`.
#[derive(Debug)]
pub struct FindRequest {
    /// Nonce.
    pub identifier: u32,
    /// Adresse LAN du demandeur (pour le puncture).
    pub lan_address: UdpAddress,
    /// Cle cible.
    pub target: [u8; 20],
    /// Offset de pagination des valeurs.
    pub offset: u32,
    /// Forcer la recherche de noeuds meme si des valeurs existent.
    pub force_nodes: bool,
}

impl FindRequest {
    pub(crate) fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.identifier);
        w.ip_address(&self.lan_address)?;
        w.bytes(&self.target);
        w.u32(self.offset);
        w.u8(self.force_nodes as u8);
        Ok(())
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            lan_address: r.ip_address()?,
            target: r.take(20)?.try_into().unwrap(),
            offset: r.u32()?,
            force_nodes: r.u8()? != 0,
        })
    }
}

/// `FindResponsePayload` : `I, 20s, varlenH-list, node-list`.
#[derive(Debug)]
pub struct FindResponse {
    /// Nonce en echo.
    pub identifier: u32,
    /// Jeton pour un futur store-request.
    pub token: [u8; 20],
    /// Valeurs trouvees (serialisees).
    pub values: Vec<Vec<u8>>,
    /// Noeuds proches connus (adresse, cle publique).
    pub nodes: Vec<(UdpAddress, Vec<u8>)>,
}

impl FindResponse {
    pub(crate) fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.identifier);
        w.bytes(&self.token);
        write_varlen_h_list(w, &self.values);
        write_node_list(w, &self.nodes)?;
        Ok(())
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            token: r.take(20)?.try_into().unwrap(),
            values: read_varlen_h_list(r)?,
            nodes: read_node_list(r)?,
        })
    }
}

/// `StorePeerRequestPayload` : `I, 20s, 20s`.
#[derive(Debug)]
pub struct StorePeerRequest {
    /// Nonce.
    pub identifier: u32,
    /// Jeton.
    pub token: [u8; 20],
    /// Cle = mid du pair a enregistrer.
    pub target: [u8; 20],
}

impl StorePeerRequest {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
        w.bytes(&self.token);
        w.bytes(&self.target);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            token: r.take(20)?.try_into().unwrap(),
            target: r.take(20)?.try_into().unwrap(),
        })
    }
}

/// `StorePeerResponsePayload` : `I`.
#[derive(Debug)]
pub struct StorePeerResponse {
    /// Nonce en echo.
    pub identifier: u32,
}

impl StorePeerResponse {
    pub(crate) fn pack(&self, w: &mut Writer) {
        w.u32(self.identifier);
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
        })
    }
}

/// `ConnectPeerRequestPayload` : `I, ip_address, 20s`.
#[derive(Debug)]
pub struct ConnectPeerRequest {
    /// Nonce.
    pub identifier: u32,
    /// Adresse LAN du demandeur.
    pub lan_address: UdpAddress,
    /// MID du pair recherche.
    pub target: [u8; 20],
}

impl ConnectPeerRequest {
    pub(crate) fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.identifier);
        w.ip_address(&self.lan_address)?;
        w.bytes(&self.target);
        Ok(())
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            lan_address: r.ip_address()?,
            target: r.take(20)?.try_into().unwrap(),
        })
    }
}

/// `ConnectPeerResponsePayload` : `I, node-list`.
#[derive(Debug)]
pub struct ConnectPeerResponse {
    /// Nonce en echo.
    pub identifier: u32,
    /// Pairs qui hebergent la cible.
    pub nodes: Vec<(UdpAddress, Vec<u8>)>,
}

impl ConnectPeerResponse {
    pub(crate) fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u32(self.identifier);
        write_node_list(w, &self.nodes)?;
        Ok(())
    }
    pub(crate) fn unpack(r: &mut Reader<'_>) -> Result<Self, Ipv8Error> {
        Ok(Self {
            identifier: r.u32()?,
            nodes: read_node_list(r)?,
        })
    }
}

/// `serialize_value` : blob a stocker sous une cle DHT.
///
/// - non signe : `0x00 + raw(data)` ;
/// - signe : `0x01 + varlenH(data) + I(version) + varlenH(pubkey) + sig`
///   (signature Ed25519 sur tout ce qui precede — `_ez_pack(b"", …)`).
pub fn serialize_value(data: &[u8], sign: bool, key: &LibNaClSecretKey) -> Vec<u8> {
    if !sign {
        let mut w = Writer::new();
        w.u8(DHT_ENTRY_STR);
        w.raw(data);
        return w.into_bytes();
    }
    let mut w = Writer::new();
    w.u8(DHT_ENTRY_STR_SIGNED);
    w.varlen_h(data);
    let version = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    w.u32(version);
    w.varlen_h(&key.public_key().to_bin());
    let mut out = w.into_bytes();
    let sig = key.sign(&out);
    out.extend_from_slice(&sig);
    out
}

/// `unserialize_value` : extrait `(data, public_key, version)` d'un
/// blob stocke. `None` si le format ou la signature est invalide.
pub fn unserialize_value(value: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>, u32)> {
    let (&first, rest) = value.split_first()?;
    match first {
        DHT_ENTRY_STR => Some((rest.to_vec(), None, 0)),
        DHT_ENTRY_STR_SIGNED => {
            if rest.len() < SIGNATURE_LENGTH {
                return None;
            }
            let signed = &rest[..rest.len() - SIGNATURE_LENGTH];
            let sig = &rest[rest.len() - SIGNATURE_LENGTH..];
            let mut r = Reader::new(signed);
            let data = r.varlen_h().ok()?.to_vec();
            let version = r.u32().ok()?;
            let public_key = r.varlen_h().ok()?.to_vec();
            let pk = LibNaClPublicKey::from_bin(&public_key).ok()?;
            // La signature couvre `0x01 + SignedStrPayload`, soit
            // `value[..-64]` dans son ensemble.
            if !pk.verify(&value[..value.len() - SIGNATURE_LENGTH], sig) {
                return None;
            }
            Some((data, Some(public_key), version))
        }
        _ => None,
    }
}
