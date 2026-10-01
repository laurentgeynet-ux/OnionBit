// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blobs de metadonnees de canaux Tribler (`.mdblob`).
//!
//! Reference de verite :
//! `D:\Projet\Tribler_sources\tribler\src\tribler\core\database\serialization.py`.
//!
//! Un fichier `.mdblob` est une **sequence** de `SignedPayload` au format
//! filaire pyipv8 :
//!
//! ```text
//! H  metadata_type   (2 octets, big-endian)
//! H  reserved_flags  (2 octets)
//! 64s public_key     (64 octets = key_to_bin()[10:] d'une LibNaCLPK)
//! ... champs specifiques au type ...
//! 64s signature      (Ed25519 sur tous les octets precedents)
//! ```
//!
//! Types de metadonnees (cf. `serialization.py`) : `CHANNEL_NODE=200`,
//! `METADATA_NODE=210`, `COLLECTION_NODE=220`, `JSON_NODE=230`,
//! `CHANNEL_DESCRIPTION=231`, `BINARY_NODE=240`, `CHANNEL_THUMBNAIL=241`,
//! `REGULAR_TORRENT=300`, `CHANNEL_TORRENT=400`, `DELETED=500`,
//! `SNIPPET=600`.
//!
//! Seuls `REGULAR_TORRENT` et `CHANNEL_TORRENT` sont reellement utiles
//! (les autres types sont marques "deprecated" dans la reference) ; les
//! autres sont parsees en en-tete seulement.

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};

use crate::error::{FormatError, Result};

/// Types de metadonnees Tribler (equivalents des constantes Python).
pub mod types {
    pub const CHANNEL_NODE: u16 = 200;
    pub const METADATA_NODE: u16 = 210;
    pub const COLLECTION_NODE: u16 = 220;
    pub const JSON_NODE: u16 = 230;
    pub const CHANNEL_DESCRIPTION: u16 = 231;
    pub const BINARY_NODE: u16 = 240;
    pub const CHANNEL_THUMBNAIL: u16 = 241;
    pub const REGULAR_TORRENT: u16 = 300;
    pub const CHANNEL_TORRENT: u16 = 400;
    pub const DELETED: u16 = 500;
    pub const SNIPPET: u16 = 600;
}

/// Taille de la cle publique brute (sans le prefixe `LibNaCLPK:`).
const PUBLIC_KEY_LEN: usize = 64;
/// Taille d'une signature Ed25519.
const SIGNATURE_LEN: usize = 64;

/// En-tete commun de tout payload signe.
#[derive(Debug, Clone)]
pub struct SignedPayloadHeader {
    /// Type de metadonnee (`types::*`).
    pub metadata_type: u16,
    /// Flags reserves (a 0).
    pub reserved_flags: u16,
    /// Cle publique du signataire (64 octets bruts, sans prefixe).
    pub public_key: [u8; PUBLIC_KEY_LEN],
    /// Signature Ed25519 sur les octets serialises.
    pub signature: [u8; SIGNATURE_LEN],
    /// Octets serialises ayant ete signes (pour verification).
    signed_data: Vec<u8>,
}

/// Metadonnees de noeud de canal (`ChannelNodePayload`).
#[derive(Debug, Clone)]
pub struct ChannelNodePayload {
    /// En-tete signe.
    pub header: SignedPayloadHeader,
    /// Identifiant unique de l'entree (`id_`).
    pub id: u64,
    /// Identifiant du parent (`origin_id`, 0 = racine).
    pub origin_id: u64,
    /// Timestamp Unix de creation.
    pub timestamp: u64,
}

/// Metadonnees d'un torrent (`TorrentMetadataPayload`, type 300).
#[derive(Debug, Clone)]
pub struct TorrentMetadataPayload {
    /// En-tete + champs de noeud.
    pub node: ChannelNodePayload,
    /// Info-hash v1 (20 octets).
    pub infohash: [u8; 20],
    /// Taille du contenu en octets.
    pub size: u64,
    /// Date du torrent (timestamp Unix).
    pub torrent_date: u32,
    /// Titre.
    pub title: String,
    /// Tags.
    pub tags: String,
    /// Tracker (`tracker_info`).
    pub tracker_info: String,
}

/// Metadonnees d'un canal (`ChannelMetadataPayload`, type 400).
#[derive(Debug, Clone)]
pub struct ChannelMetadataPayload {
    /// Champs de torrent herites (infohash = cle publique du canal).
    pub torrent: TorrentMetadataPayload,
    /// Nombre d'entrees du canal.
    pub num_entries: u64,
    /// Timestamp de debut du canal.
    pub start_timestamp: u64,
}

/// `COLLECTION_NODE` (220) : noeud de canal + titre/tags + nombre
/// d'entrees. Deprecated cote Python mais encore serialise.
#[derive(Debug, Clone)]
pub struct CollectionNodePayload {
    /// Champs de noeud de canal.
    pub node: ChannelNodePayload,
    /// Titre.
    pub title: String,
    /// Tags.
    pub tags: String,
    /// Nombre d'entrees de la collection.
    pub num_entries: u64,
}

/// `JsonNodePayload` (types 230 `JSON_NODE` et 231 `CHANNEL_DESCRIPTION`).
#[derive(Debug, Clone)]
pub struct JsonNodePayload {
    /// Champs de noeud de canal.
    pub node: ChannelNodePayload,
    /// Contenu JSON (texte).
    pub json_text: String,
}

/// `BinaryNodePayload` (types 240 `BINARY_NODE` et 241 `CHANNEL_THUMBNAIL`).
#[derive(Debug, Clone)]
pub struct BinaryNodePayload {
    /// Champs de noeud de canal.
    pub node: ChannelNodePayload,
    /// Donnees binaires.
    pub binary_data: Vec<u8>,
    /// Type MIME de la donnee.
    pub data_type: String,
}

/// `DeletedMetadataPayload` (type 500) : entree marquee supprimee.
#[derive(Debug, Clone)]
pub struct DeletedPayload {
    /// En-tete signe.
    pub header: SignedPayloadHeader,
    /// Signature de la suppression.
    pub delete_signature: [u8; SIGNATURE_LEN],
}

/// Entree de blob parsee (union des types connus du mapping Python
/// `METADATA_TYPE_TO_PAYLOAD_CLASS`).
#[derive(Debug, Clone)]
pub enum MetadataEntry {
    /// `REGULAR_TORRENT` (300).
    RegularTorrent(TorrentMetadataPayload),
    /// `CHANNEL_TORRENT` (400).
    ChannelTorrent(ChannelMetadataPayload),
    /// `COLLECTION_NODE` (220).
    CollectionNode(CollectionNodePayload),
    /// `JSON_NODE` (230) ou `CHANNEL_DESCRIPTION` (231).
    JsonNode(JsonNodePayload),
    /// `BINARY_NODE` (240) ou `CHANNEL_THUMBNAIL` (241).
    BinaryNode(BinaryNodePayload),
    /// `DELETED` (500).
    Deleted(DeletedPayload),
    /// `CHANNEL_NODE` (200) ou `METADATA_NODE` (210) ou `SNIPPET` (600) —
    /// types presents dans les constantes Python mais absents du mapping
    /// de parsing officiel : rejetes comme `UnknownBlobTypeException`.
    Rejected {
        /// Type rencontre.
        metadata_type: u16,
    },
}

impl MetadataEntry {
    /// En-tete signe de l'entree (tous types confondus, hors `Rejected`).
    pub fn header(&self) -> Option<&SignedPayloadHeader> {
        match self {
            MetadataEntry::RegularTorrent(t) => Some(&t.node.header),
            MetadataEntry::ChannelTorrent(t) => Some(&t.torrent.node.header),
            MetadataEntry::CollectionNode(t) => Some(&t.node.header),
            MetadataEntry::JsonNode(t) => Some(&t.node.header),
            MetadataEntry::BinaryNode(t) => Some(&t.node.header),
            MetadataEntry::Deleted(t) => Some(&t.header),
            MetadataEntry::Rejected { .. } => None,
        }
    }
}

impl TorrentMetadataPayload {
    /// Lien magnet equivalent (equivalent de `get_magnet()` Python).
    pub fn magnet(&self) -> String {
        let mut m = format!(
            "magnet:?xt=urn:btih:{}&dn={}",
            onionbit_crypto::hash::to_hex(&self.infohash),
            self.title
        );
        if !self.tracker_info.is_empty() {
            m.push_str(&format!("&tr={}", self.tracker_info));
        }
        m
    }
}

impl SignedPayloadHeader {
    /// Construit un en-tete pour serialisation (signature a zero —
    /// remplacee par [`encode_entry`]/[`encode_entry_presigned`]).
    pub fn new(metadata_type: u16, reserved_flags: u16, public_key: [u8; PUBLIC_KEY_LEN]) -> Self {
        Self {
            metadata_type,
            reserved_flags,
            public_key,
            signature: [0u8; SIGNATURE_LEN],
            signed_data: Vec::new(),
        }
    }

    /// Definit la signature (re-serialisation d'une entree recue).
    pub fn with_signature(mut self, signature: [u8; SIGNATURE_LEN]) -> Self {
        self.signature = signature;
        self
    }

    /// Verifie la signature Ed25519 du payload.
    ///
    /// La cle se reconstruit comme cote Python :
    /// `key_from_public_bin(b"LibNaCLPK:" + public_key)`.
    pub fn verify_signature(&self) -> bool {
        let mut key_bin = Vec::with_capacity(10 + PUBLIC_KEY_LEN);
        key_bin.extend_from_slice(b"LibNaCLPK:");
        key_bin.extend_from_slice(&self.public_key);
        let Ok(pk) = LibNaClPublicKey::from_bin(&key_bin) else {
            return false;
        };
        pk.verify(&self.signed_data, &self.signature)
    }
}

// --- Lecteurs bornes de champs pyipv8 sur un curseur -------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(FormatError::Truncated { offset: self.pos })?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(FormatError::Truncated { offset: self.pos })?;
        self.pos = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// `varlenI` : longueur u32 + octets.
    fn varlen_i(&mut self) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    /// `varlenIutf8` : `varlenI` decode en UTF-8 (perte admise).
    fn varlen_i_utf8(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.varlen_i()?).into_owned())
    }
}

/// Parse une entree de blob `.mdblob` a partir de `data`, en partant de
/// `offset`. Retourne `(entree, nouvel_offset)`.
///
/// Equivalent de `read_payload_with_offset` cote Python : les types
/// absents de `METADATA_TYPE_TO_PAYLOAD_CLASS` levent
/// `UnknownBlobTypeException` ; ici ils retournent
/// [`MetadataEntry::Rejected`].
pub fn read_entry(data: &[u8], offset: usize) -> Result<(MetadataEntry, usize)> {
    let mut c = Cursor::new(&data[offset..]);
    let metadata_type = c.u16()?;
    let reserved_flags = c.u16()?;
    let mut public_key = [0u8; PUBLIC_KEY_LEN];
    public_key.copy_from_slice(c.take(PUBLIC_KEY_LEN)?);

    /// Lit les trois champs Q communs a tout `ChannelNodePayload`.
    fn channel_node_fields(c: &mut Cursor<'_>) -> Result<(u64, u64, u64)> {
        Ok((c.u64()?, c.u64()?, c.u64()?))
    }

    let entry = match metadata_type {
        types::REGULAR_TORRENT | types::CHANNEL_TORRENT => {
            let (id, origin_id, timestamp) = channel_node_fields(&mut c)?;
            let mut infohash = [0u8; 20];
            infohash.copy_from_slice(c.take(20)?);
            let torrent = TorrentMetadataPayload {
                node: ChannelNodePayload {
                    header: SignedPayloadHeader {
                        metadata_type,
                        reserved_flags,
                        public_key,
                        signature: [0u8; 64],
                        signed_data: Vec::new(),
                    },
                    id,
                    origin_id,
                    timestamp,
                },
                infohash,
                size: c.u64()?,
                torrent_date: c.u32()?,
                title: c.varlen_i_utf8()?,
                tags: c.varlen_i_utf8()?,
                tracker_info: c.varlen_i_utf8()?,
            };
            if metadata_type == types::CHANNEL_TORRENT {
                MetadataEntry::ChannelTorrent(ChannelMetadataPayload {
                    torrent,
                    num_entries: c.u64()?,
                    start_timestamp: c.u64()?,
                })
            } else {
                MetadataEntry::RegularTorrent(torrent)
            }
        }
        types::COLLECTION_NODE => {
            let (id, origin_id, timestamp) = channel_node_fields(&mut c)?;
            MetadataEntry::CollectionNode(CollectionNodePayload {
                node: ChannelNodePayload {
                    header: SignedPayloadHeader {
                        metadata_type,
                        reserved_flags,
                        public_key,
                        signature: [0u8; 64],
                        signed_data: Vec::new(),
                    },
                    id,
                    origin_id,
                    timestamp,
                },
                title: c.varlen_i_utf8()?,
                tags: c.varlen_i_utf8()?,
                num_entries: c.u64()?,
            })
        }
        types::JSON_NODE | types::CHANNEL_DESCRIPTION => {
            let (id, origin_id, timestamp) = channel_node_fields(&mut c)?;
            MetadataEntry::JsonNode(JsonNodePayload {
                node: ChannelNodePayload {
                    header: SignedPayloadHeader {
                        metadata_type,
                        reserved_flags,
                        public_key,
                        signature: [0u8; 64],
                        signed_data: Vec::new(),
                    },
                    id,
                    origin_id,
                    timestamp,
                },
                json_text: c.varlen_i_utf8()?,
            })
        }
        types::BINARY_NODE | types::CHANNEL_THUMBNAIL => {
            let (id, origin_id, timestamp) = channel_node_fields(&mut c)?;
            MetadataEntry::BinaryNode(BinaryNodePayload {
                node: ChannelNodePayload {
                    header: SignedPayloadHeader {
                        metadata_type,
                        reserved_flags,
                        public_key,
                        signature: [0u8; 64],
                        signed_data: Vec::new(),
                    },
                    id,
                    origin_id,
                    timestamp,
                },
                binary_data: c.varlen_i()?.to_vec(),
                data_type: c.varlen_i_utf8()?,
            })
        }
        types::DELETED => {
            let mut delete_signature = [0u8; SIGNATURE_LEN];
            delete_signature.copy_from_slice(c.take(SIGNATURE_LEN)?);
            MetadataEntry::Deleted(DeletedPayload {
                header: SignedPayloadHeader {
                    metadata_type,
                    reserved_flags,
                    public_key,
                    signature: [0u8; 64],
                    signed_data: Vec::new(),
                },
                delete_signature,
            })
        }
        // Types definis en constantes mais absents du mapping Python
        // (CHANNEL_NODE 200, METADATA_NODE 210, SNIPPET 600) ou totalement
        // inconnus : equivalents de `UnknownBlobTypeException`.
        other => {
            return Ok((
                MetadataEntry::Rejected {
                    metadata_type: other,
                },
                offset + c.pos,
            ))
        }
    };

    // Tous les types connus se terminent par la signature Ed25519 (64o).
    let signed_len = c.pos;
    let signature = c.take(SIGNATURE_LEN)?;
    let signed_data = data[offset..offset + signed_len].to_vec();
    let mut sig = [0u8; SIGNATURE_LEN];
    sig.copy_from_slice(signature);

    let entry = match entry {
        MetadataEntry::RegularTorrent(mut t) => {
            t.node.header.signature = sig;
            t.node.header.signed_data = signed_data;
            MetadataEntry::RegularTorrent(t)
        }
        MetadataEntry::ChannelTorrent(mut t) => {
            t.torrent.node.header.signature = sig;
            t.torrent.node.header.signed_data = signed_data;
            MetadataEntry::ChannelTorrent(t)
        }
        MetadataEntry::CollectionNode(mut t) => {
            t.node.header.signature = sig;
            t.node.header.signed_data = signed_data;
            MetadataEntry::CollectionNode(t)
        }
        MetadataEntry::JsonNode(mut t) => {
            t.node.header.signature = sig;
            t.node.header.signed_data = signed_data;
            MetadataEntry::JsonNode(t)
        }
        MetadataEntry::BinaryNode(mut t) => {
            t.node.header.signature = sig;
            t.node.header.signed_data = signed_data;
            MetadataEntry::BinaryNode(t)
        }
        MetadataEntry::Deleted(mut t) => {
            t.header.signature = sig;
            t.header.signed_data = signed_data;
            MetadataEntry::Deleted(t)
        }
        MetadataEntry::Rejected { .. } => unreachable!("Rejected retourne avant la signature"),
    };

    Ok((entry, offset + c.pos))
}

// --- Serialisation ----------------------------------------------------

/// Ecrit les champs communs `ChannelNodePayload` (type + flags + cle +
/// `id`/`origin_id`/`timestamp`).
fn write_node_header(out: &mut Vec<u8>, node: &ChannelNodePayload) {
    out.extend_from_slice(&node.header.metadata_type.to_be_bytes());
    out.extend_from_slice(&node.header.reserved_flags.to_be_bytes());
    out.extend_from_slice(&node.header.public_key);
    out.extend_from_slice(&node.id.to_be_bytes());
    out.extend_from_slice(&node.origin_id.to_be_bytes());
    out.extend_from_slice(&node.timestamp.to_be_bytes());
}

/// `varlenI` : longueur u32 + octets.
fn write_varlen_i(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

fn write_torrent_fields(out: &mut Vec<u8>, t: &TorrentMetadataPayload) {
    out.extend_from_slice(&t.infohash);
    out.extend_from_slice(&t.size.to_be_bytes());
    out.extend_from_slice(&t.torrent_date.to_be_bytes());
    write_varlen_i(out, t.title.as_bytes());
    write_varlen_i(out, t.tags.as_bytes());
    write_varlen_i(out, t.tracker_info.as_bytes());
}

/// Serialise une entree `.mdblob` **sans la signature** : retourne les
/// octets a signer (la signature Ed25519 couvre tout le payload,
/// comme `SignedPayload` cote pyipv8).
pub fn encode_entry_unsigned(entry: &MetadataEntry) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match entry {
        MetadataEntry::RegularTorrent(t) => {
            write_node_header(&mut out, &t.node);
            write_torrent_fields(&mut out, t);
        }
        MetadataEntry::ChannelTorrent(c) => {
            write_node_header(&mut out, &c.torrent.node);
            write_torrent_fields(&mut out, &c.torrent);
            out.extend_from_slice(&c.num_entries.to_be_bytes());
            out.extend_from_slice(&c.start_timestamp.to_be_bytes());
        }
        MetadataEntry::CollectionNode(c) => {
            write_node_header(&mut out, &c.node);
            write_varlen_i(&mut out, c.title.as_bytes());
            write_varlen_i(&mut out, c.tags.as_bytes());
            out.extend_from_slice(&c.num_entries.to_be_bytes());
        }
        MetadataEntry::JsonNode(j) => {
            write_node_header(&mut out, &j.node);
            write_varlen_i(&mut out, j.json_text.as_bytes());
        }
        MetadataEntry::BinaryNode(b) => {
            write_node_header(&mut out, &b.node);
            write_varlen_i(&mut out, &b.binary_data);
            write_varlen_i(&mut out, b.data_type.as_bytes());
        }
        MetadataEntry::Deleted(d) => {
            out.extend_from_slice(&d.header.metadata_type.to_be_bytes());
            out.extend_from_slice(&d.header.reserved_flags.to_be_bytes());
            out.extend_from_slice(&d.header.public_key);
            out.extend_from_slice(&d.delete_signature);
        }
        MetadataEntry::Rejected { metadata_type } => {
            return Err(FormatError::BadBencode {
                offset: 0,
                reason: format!("type de metadonnee non serialisable: {metadata_type}"),
            });
        }
    }
    Ok(out)
}

/// Serialise et **signe** une entree `.mdblob` (signature Ed25519
/// sur tous les octets precedents).
pub fn encode_entry(entry: &MetadataEntry, signer: &LibNaClSecretKey) -> Result<Vec<u8>> {
    let mut out = encode_entry_unsigned(entry)?;
    let sig = signer.sign(&out);
    out.extend_from_slice(&sig);
    Ok(out)
}

/// Re-ecrit une entree deja signee (signature deja presente dans
/// l'en-tete — propagation d'entrees distantes, `entries_to_chunk`
/// cote Python).
pub fn encode_entry_presigned(entry: &MetadataEntry) -> Result<Vec<u8>> {
    let mut out = encode_entry_unsigned(entry)?;
    let sig = entry
        .header()
        .map(|h| h.signature)
        .unwrap_or([0u8; SIGNATURE_LEN]);
    out.extend_from_slice(&sig);
    Ok(out)
}

/// Parse toutes les entrees d'un blob `.mdblob` (sequence de payloads).
///
/// Echoue sur le premier type inconnu (equivalent de
/// `UnknownBlobTypeException` cote Python).
pub fn parse_blob(data: &[u8]) -> Result<Vec<MetadataEntry>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (entry, next) = read_entry(data, offset)?;
        if next <= offset {
            return Err(FormatError::BadBencode {
                offset,
                reason: "entree de blob de taille nulle (boucle infinie)".into(),
            });
        }
        if let MetadataEntry::Rejected { metadata_type } = entry {
            return Err(FormatError::BadBencode {
                offset,
                reason: format!("type de metadonnee inconnu: {metadata_type}"),
            });
        }
        out.push(entry);
        offset = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;

    /// Construit un blob .mdblob a une entree REGULAR_TORRENT signee.
    fn build_blob(sk: &LibNaClSecretKey) -> Vec<u8> {
        let pk_bin = sk.public_key().to_bin();
        let mut payload = Vec::new();
        payload.extend_from_slice(&types::REGULAR_TORRENT.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes()); // reserved_flags
        payload.extend_from_slice(&pk_bin[10..]); // public_key (64o)
        payload.extend_from_slice(&1u64.to_be_bytes()); // id
        payload.extend_from_slice(&0u64.to_be_bytes()); // origin_id
        payload.extend_from_slice(&1_700_000_000u64.to_be_bytes()); // timestamp
        payload.extend_from_slice(&[7u8; 20]); // infohash
        payload.extend_from_slice(&1234u64.to_be_bytes()); // size
        payload.extend_from_slice(&1_700_000_000u32.to_be_bytes()); // torrent_date
        let title = b"titre test";
        payload.extend_from_slice(&(title.len() as u32).to_be_bytes());
        payload.extend_from_slice(title);
        payload.extend_from_slice(&0u32.to_be_bytes()); // tags vide
        let tr = b"udp://t.local";
        payload.extend_from_slice(&(tr.len() as u32).to_be_bytes());
        payload.extend_from_slice(tr);
        let sig = sk.sign(&payload);
        payload.extend_from_slice(&sig);
        payload
    }

    #[test]
    fn parse_blob_regular_torrent() {
        let sk = LibNaClSecretKey::generate();
        let blob = build_blob(&sk);
        let entries = parse_blob(&blob).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            MetadataEntry::RegularTorrent(t) => {
                assert_eq!(t.node.header.metadata_type, types::REGULAR_TORRENT);
                assert_eq!(t.infohash, [7u8; 20]);
                assert_eq!(t.size, 1234);
                assert_eq!(t.title, "titre test");
                assert_eq!(t.tracker_info, "udp://t.local");
                assert!(t.node.header.verify_signature());
            }
            _ => panic!("entree attendue RegularTorrent"),
        }
    }

    #[test]
    fn signature_invalide_detectee() {
        let sk = LibNaClSecretKey::generate();
        let other = LibNaClSecretKey::generate();
        let mut blob = build_blob(&sk);
        // Resigner avec une autre cle -> signature invalide pour la cle embarquee.
        let payload_len = blob.len() - SIGNATURE_LEN;
        let bad_sig = other.sign(&blob[..payload_len]);
        blob[payload_len..].copy_from_slice(&bad_sig);
        let entries = parse_blob(&blob).unwrap();
        match &entries[0] {
            MetadataEntry::RegularTorrent(t) => assert!(!t.node.header.verify_signature()),
            _ => panic!("entree attendue RegularTorrent"),
        }
    }

    #[test]
    fn parse_blob_rejette_type_inconnu() {
        let header_len = 2 + 2 + PUBLIC_KEY_LEN;
        let mut blob = vec![0u8; header_len + SIGNATURE_LEN];
        blob[..2].copy_from_slice(&999u16.to_be_bytes());
        assert!(parse_blob(&blob).is_err());
    }
}
