//! Paquet IPv8 signe (equivalent de `community.py::_ez_pack` /
//! `lazy_community.py::_ez_unpack_auth`).
//!
//! Layout sur le fil :
//!
//! ```text
//! [0..22)  prefix = 0x00 + version(1o, 0x02) + community_id(20o)
//! [22]     msg_id (1 octet)
//! [23..]   auth  = varlenH(public_key_bin)   (74 octets utiles)
//!          dist  = Q(global_time)            (8 octets)
//!          payload serialise
//! [fin-64] signature Ed25519 sur tout le paquet precedent
//! ```

use tribler_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, SIGNATURE_LENGTH};

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

/// Paquet IPv8 decode (signature deja verifiee).
#[derive(Debug)]
pub struct Packet {
    /// Community destinataire (extraite du prefixe).
    pub community_id: CommunityId,
    /// Identifiant du message dans la community.
    pub msg_id: u8,
    /// Cle publique binaire de l'emetteur (`LibNaClPK:…`).
    pub public_key_bin: Vec<u8>,
    /// Horodatage de Lamport (`GlobalTimeDistributionPayload`).
    pub global_time: u64,
    /// Octets du payload applicatif (apres auth+dist, avant signature).
    pub payload: Vec<u8>,
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

    /// Decode et verifie la signature d'un paquet recu.
    ///
    /// `expected` peut etre `None` pour accepter n'importe quel prefixe
    /// (dispatcher) ; sinon le paquet est rejete si le prefixe ne
    /// correspond pas (comme `on_packet` Python).
    pub fn parse(data: &[u8], expected: Option<&CommunityId>) -> Result<Self, Ipv8Error> {
        if data.len() < PREFIX_LEN + 1 + SIGNATURE_LENGTH {
            return Err(Ipv8Error::Truncated {
                need: PREFIX_LEN + 1 + SIGNATURE_LENGTH,
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

        // Signature = 64 derniers octets ; porte sur tout le reste.
        let sig = &data[data.len() - SIGNATURE_LENGTH..];
        let signed = &data[..data.len() - SIGNATURE_LENGTH];

        // Le reader est borne a `signed` (sans la signature) pour que
        // `raw` n'avale pas les 64 derniers octets.
        let mut r = Reader::new(&signed[23..]);
        let public_key_bin = r.varlen_h()?.to_vec();
        let global_time = r.u64()?;
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
