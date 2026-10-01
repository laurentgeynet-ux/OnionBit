// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Paquet IPv8 signe (equivalent de `community.py::_ez_pack` /
//! `lazy_community.py::_ez_unpack_auth`).
//!
//! Layout sur le fil :
//!
//! ```text
//! [0..22)  prefix = 0x00 + version(1o, 0x02) + community_id(20o)
//! [22]     msg_id (1 octet)
//! [23..]   auth  = varlenH(public_key_bin)   (74 octets utiles)
//!          dist  = Q(global_time)            (8 octets, uniquement
//!                                              pour `DIST_MSG_IDS` :
//!                                              intros/punctures)
//!          payload serialise
//! [fin-64] signature Ed25519 sur tout le paquet precedent
//! ```

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, SIGNATURE_LENGTH};

use crate::address::UdpAddress;
use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};
use crate::CommunityId;

/// Longueur du prefixe : `0x00` + version + community_id.
pub const PREFIX_LEN: usize = 1 + 1 + 20;

/// Version du protocole (champ `version` des communities pyipv8).
pub const PROTOCOL_VERSION: u8 = 0x02;

/// Prefixe complet d'une community (22 octets).
pub fn prefix_of(community_id: &CommunityId) -> [u8; PREFIX_LEN] {
    let mut p = [0u8; PREFIX_LEN];
    p[1] = PROTOCOL_VERSION;
    p[2..].copy_from_slice(community_id);
    p
}

/// `msg_id` IPv8 historiquement **non signes** (cf.
/// `lazy_wrapper_unsigned` pyipv8) : les requetes de puncture (anciennes
/// `250` et nouvelles `232`) ne portent ni auth ni signature — le paquet
/// est `prefix + msg_id + Q(global_time) + payload`.
pub const UNSIGNED_MSG_IDS: &[u8] = &[250, 232];

/// `msg_id` des messages **signes** dont le corps commence par
/// `GlobalTimeDistributionPayload` (`Q(global_time)`, 8 octets) : les
/// introductions et punctures signees, construites dans pyipv8 par
/// `create_introduction_*`/`create_puncture` via
/// `_ez_pack(prefix, msg_id, [auth, dist, payload])`.
///
/// Tous les autres messages signes sont emis par `ez_send`
/// (`ezr_pack`) = `prefix + msg_id + varlenH(pubkey) + payload`,
/// **sans** `dist` — c'est le cas du DHT (`PingRequest`…) et des
/// cellules tunnel.
pub const DIST_MSG_IDS: &[u8] = &[246, 245, 234, 233, 249, 231];

/// Politique de layout filaire d'une community : la presence de
/// `auth`/`dist` ne depend pas du `msg_id` seul mais de la liste `fmt`
/// du handler Python (`lazy_wrapper(GlobalTimeDistributionPayload, X)`
/// = dist ; `lazy_wrapper_unsigned` = non signe). Le meme `msg_id` peut
/// donc avoir des layouts differents selon la community (ex. `4` =
/// `Pong` non signe en discovery, `Health` signe sans dist en
/// content-discovery, cellule `extend` sur le prefixe tunnel).
#[derive(Debug, Clone, Copy)]
pub struct WirePolicy {
    /// `msg_id` non signes (`lazy_wrapper_unsigned`) : le paquet est
    /// `prefix + msg_id + Q(global_time) + payload`.
    pub unsigned: &'static [u8],
    /// `msg_id` signes portant `GlobalTimeDistributionPayload`
    /// (`Q(global_time)` juste apres l'auth).
    pub dist: &'static [u8],
}

impl WirePolicy {
    /// `true` si le message n'est ni signe ni authentifie.
    pub fn is_unsigned(&self, msg_id: u8) -> bool {
        self.unsigned.contains(&msg_id)
    }

    /// `true` si le message signe porte `GlobalTimeDistributionPayload`.
    pub fn has_dist(&self, msg_id: u8) -> bool {
        self.dist.contains(&msg_id)
    }
}

/// Politique par defaut : intros/punctures signees avec `dist`,
/// puncture-requests non signees (commun aux overlays DHT,
/// content-discovery et tunnel).
pub const WIRE_DEFAULT: WirePolicy = WirePolicy {
    unsigned: UNSIGNED_MSG_IDS,
    dist: DIST_MSG_IDS,
};

/// `DiscoveryCommunity` pyipv8 : similarity (1/2), introductions et
/// punctures signes **avec** `dist` (`lazy_wrapper` avec
/// `GlobalTimeDistributionPayload`) ; ping/pong (3/4) et
/// puncture-requests non signes **avec** `dist`
/// (`_ez_pack(..., [dist, payload], False)`).
pub const WIRE_DISCOVERY: WirePolicy = WirePolicy {
    unsigned: &[3, 4, 250, 232],
    dist: &[1, 2, 246, 245, 234, 233, 249, 231],
};

/// Paquet IPv8 decode (signature deja verifiee si `signed`).
#[derive(Debug)]
pub struct Packet {
    /// Community destinataire (extraite du prefixe).
    pub community_id: CommunityId,
    /// Identifiant du message dans la community.
    pub msg_id: u8,
    /// Cle publique binaire de l'emetteur (`LibNaClPK:…`), vide si non
    /// signe.
    pub public_key_bin: Vec<u8>,
    /// Horodatage de Lamport (`GlobalTimeDistributionPayload`) — `0`
    /// pour les messages signes `ez_send` qui n'en portent pas.
    pub global_time: u64,
    /// Octets du payload applicatif (apres auth+dist, avant signature).
    pub payload: Vec<u8>,
    /// `true` si le paquet portait auth + signature Ed25519 valide.
    /// `false` pour les messages historiquement non signes
    /// (`UNSIGNED_MSG_IDS`) — l'emetteur **ne doit pas** etre marque
    /// pair verifie.
    pub signed: bool,
}

impl Packet {
    /// Construit et signe un paquet : `prefix + msg_id + varlenH(pub) +
    /// Q(gtime) + payload`, puis signature Ed25519 de l'ensemble.
    pub fn sign(
        community_id: &CommunityId,
        msg_id: u8,
        key: &LibNaClSecretKey,
        global_time: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&prefix_of(community_id));
        w.u8(msg_id);
        w.varlen_h(&key.public_key().to_bin());
        w.u64(global_time);
        w.raw(payload);
        let mut packet = w.into_bytes();
        let sig = key.sign(&packet);
        packet.extend_from_slice(&sig);
        packet
    }

    /// Equivalent de `ez_send`/`ezr_pack` pyipv8 : `prefix + msg_id +
    /// varlenH(pubkey) + payload` signe, **sans**
    /// `GlobalTimeDistributionPayload` — reservee aux
    /// introductions/punctures (`DIST_MSG_IDS`).
    pub fn sign_no_dist(
        community_id: &CommunityId,
        msg_id: u8,
        key: &LibNaClSecretKey,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&prefix_of(community_id));
        w.u8(msg_id);
        w.varlen_h(&key.public_key().to_bin());
        w.raw(payload);
        let mut packet = w.into_bytes();
        let sig = key.sign(&packet);
        packet.extend_from_slice(&sig);
        packet
    }

    /// Choisit le layout signe selon `msg_id` : `dist` pour les
    /// introductions/punctures (`DIST_MSG_IDS`), `ez_send` pur sinon —
    /// comme pyipv8 qui ajoute `GlobalTimeDistributionPayload` au corps
    /// uniquement dans `create_introduction_*`/`create_puncture*`.
    pub fn sign_auto(
        community_id: &CommunityId,
        msg_id: u8,
        key: &LibNaClSecretKey,
        global_time: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        if DIST_MSG_IDS.contains(&msg_id) {
            Self::sign(community_id, msg_id, key, global_time, payload)
        } else {
            Self::sign_no_dist(community_id, msg_id, key, payload)
        }
    }

    /// Construit un paquet **non signe** (ni auth ni signature) :
    /// `prefix + msg_id + Q(global_time) + payload`. Utilise par
    /// `create_puncture_request` Python (`_ez_pack(..., sig=False)`).
    pub fn pack_unsigned(
        community_id: &CommunityId,
        msg_id: u8,
        global_time: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&prefix_of(community_id));
        w.u8(msg_id);
        w.u64(global_time);
        w.raw(payload);
        w.into_bytes()
    }

    /// Decode et verifie la signature d'un paquet recu.
    ///
    /// `expected` peut etre `None` pour accepter n'importe quel prefixe
    /// (dispatcher) ; sinon le paquet est rejete si le prefixe ne
    /// correspond pas (comme `on_packet` Python). `policy` est le
    /// layout filaire de la community destinataire (`WirePolicy`).
    pub fn parse(
        data: &[u8],
        expected: Option<&CommunityId>,
        policy: &WirePolicy,
    ) -> Result<Self, Ipv8Error> {
        if data.len() < PREFIX_LEN + 1 {
            return Err(Ipv8Error::Truncated {
                need: PREFIX_LEN + 1,
                have: data.len(),
            });
        }
        if data[0] != 0x00 || data[1] != PROTOCOL_VERSION {
            return Err(Ipv8Error::Malformed("mauvais magic/version de prefixe"));
        }
        let mut community_id = [0u8; 20];
        community_id.copy_from_slice(&data[2..22]);
        if let Some(exp) = expected {
            if &community_id != exp {
                return Err(Ipv8Error::Malformed("prefixe d'une autre community"));
            }
        }
        let msg_id = data[22];

        // Messages historiquement non signes (`lazy_wrapper_unsigned`) :
        // `msg_id + Q(global_time) + payload`, sans auth ni signature.
        if policy.is_unsigned(msg_id) {
            let mut r = Reader::new(&data[23..]);
            let global_time = r.u64()?;
            let payload = r.raw().to_vec();
            return Ok(Self {
                community_id,
                msg_id,
                public_key_bin: Vec::new(),
                global_time,
                payload,
                signed: false,
            });
        }

        if data.len() < PREFIX_LEN + 1 + SIGNATURE_LENGTH {
            return Err(Ipv8Error::Truncated {
                need: PREFIX_LEN + 1 + SIGNATURE_LENGTH,
                have: data.len(),
            });
        }

        // Signature = 64 derniers octets ; porte sur tout le reste.
        let sig = &data[data.len() - SIGNATURE_LENGTH..];
        let signed = &data[..data.len() - SIGNATURE_LENGTH];

        // Le reader est borne a `signed` (sans la signature) pour que
        // `raw` n'avale pas les 64 derniers octets.
        let mut r = Reader::new(&signed[23..]);
        let public_key_bin = r.varlen_h()?.to_vec();
        // `dist` n'est present que pour les intros/punctures signees
        // (`DIST_MSG_IDS`) ; les autres messages `ez_send` enchainent
        // directement sur le payload applicatif.
        let global_time = if policy.has_dist(msg_id) { r.u64()? } else { 0 };
        let payload = r.raw().to_vec();

        // La signature couvre auth+dist+payload+prefix+msg_id, donc
        // `signed` ; la cle publique est dans l'auth.
        let pk = LibNaClPublicKey::from_bin(&public_key_bin)?;
        if !pk.verify(signed, sig) {
            return Err(Ipv8Error::InvalidSignature);
        }
        Ok(Self {
            community_id,
            msg_id,
            public_key_bin,
            global_time,
            payload,
            signed: true,
        })
    }
}

/// Helper d'envoi UDP pour une community (cf. `endpoint.send`).
pub trait PacketSender {
    /// Envoie les octets bruts a l'adresse.
    fn send_to(
        &self,
        addr: &UdpAddress,
        data: &[u8],
    ) -> impl std::future::Future<Output = Result<(), Ipv8Error>> + Send;
}
