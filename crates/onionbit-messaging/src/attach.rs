// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps des trames `attach` — descripteur de piece jointe
//! (ADR-0019 §4). Jamais de contenu de fichier dans la trame : le
//! descripteur pointe le torrent ephemere sale seede par
//! l'emetteur ; la reception est un clic explicite → download
//! anonyme classique.
//!
//! ```text
//! { "ih": <20o infohash sale>, "mid": <16o groupe de message>,
//!   "name": <nom affiche>, "size": <u64 octets> }
//! ```
//!
//! `mid` relie plusieurs pieces jointes (et le `msg` texte
//! associe) a un meme geste utilisateur — borne
//! `attach_max_per_msg` appliquee cote service.

use std::collections::BTreeMap;

use onionbit_format::bencode::{decode, BValue};

use crate::config::MessagingConfig;
use crate::error::MessagingError;

/// Taille de l'infohash BitTorrent v1.
pub const IH_LEN: usize = 20;
/// Taille du groupe de message (`mid` — dedup/regroupement).
pub const MID_LEN: usize = 16;

/// Descripteur de piece jointe (corps d'une trame `attach`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachDesc {
    /// Infohash **sale** du torrent ephemere (`x-onionbit` dans
    /// `info` — indévinable).
    pub ih: [u8; IH_LEN],
    /// Groupe de message : relie plusieurs attaches et le texte
    /// associe a un meme envoi.
    pub mid: [u8; MID_LEN],
    /// Nom affiche (borne `group_name_max_len` — meme discipline
    /// que les noms de groupe).
    pub name: String,
    /// Taille totale annoncee en octets.
    pub size: u64,
}

impl AttachDesc {
    /// Encode le corps `attach` en bencode canonique.
    pub fn encode(&self) -> Vec<u8> {
        let mut d = BTreeMap::new();
        d.insert(b"ih".to_vec(), BValue::Bytes(self.ih.to_vec()));
        d.insert(b"mid".to_vec(), BValue::Bytes(self.mid.to_vec()));
        d.insert(
            b"name".to_vec(),
            BValue::Bytes(self.name.clone().into_bytes()),
        );
        d.insert(b"size".to_vec(), BValue::Int(self.size as i64));
        BValue::Dict(d).encode()
    }

    /// Decode un corps `attach` — strict : ensemble de cles exact,
    /// tailles et bornes.
    pub fn decode_body(body: &[u8], cfg: &MessagingConfig) -> Result<Self, MessagingError> {
        let value = decode(body)?;
        let dict = value.as_dict().ok_or(MessagingError::Malformed(
            "attach : n'est pas un dictionnaire",
        ))?;
        if dict.len() != 4 {
            return Err(MessagingError::Malformed("attach : cles"));
        }
        let take = |k: &'static [u8]| -> Result<&BValue, MessagingError> {
            dict.get(k)
                .ok_or(MessagingError::Malformed("attach : cle absente"))
        };
        let ih_b = take(b"ih")?
            .as_bytes()
            .ok_or(MessagingError::Malformed("attach : ih non binaire"))?;
        let ih = <[u8; IH_LEN]>::try_from(ih_b)
            .map_err(|_| MessagingError::Malformed("attach : ih != 20 octets"))?;
        let mid_b = take(b"mid")?
            .as_bytes()
            .ok_or(MessagingError::Malformed("attach : mid non binaire"))?;
        let mid = <[u8; MID_LEN]>::try_from(mid_b)
            .map_err(|_| MessagingError::Malformed("attach : mid != 16 octets"))?;
        let name_b = take(b"name")?
            .as_bytes()
            .ok_or(MessagingError::Malformed("attach : name non binaire"))?;
        if name_b.is_empty() || name_b.len() > cfg.group_name_max_len {
            return Err(MessagingError::Malformed("attach : name hors borne"));
        }
        let name = String::from_utf8(name_b.to_vec())
            .map_err(|_| MessagingError::Malformed("attach : name non utf8"))?;
        let size = take(b"size")?
            .as_int()
            .ok_or(MessagingError::Malformed("attach : size non entier"))?;
        if size <= 0 {
            return Err(MessagingError::Malformed("attach : size <= 0"));
        }
        Ok(AttachDesc {
            ih,
            mid,
            name,
            size: size as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roundtrip du descripteur.
    #[test]
    fn attach_roundtrip() {
        let d = AttachDesc {
            ih: [3u8; 20],
            mid: [7u8; 16],
            name: "photo.png".into(),
            size: 123_456,
        };
        assert_eq!(
            AttachDesc::decode_body(&d.encode(), &MessagingConfig::default()).unwrap(),
            d
        );
    }

    /// Corps hostiles : cle en trop/absente, tailles fausses, nom
    /// hors borne, taille nulle/negative.
    #[test]
    fn attach_corps_hostiles() {
        let c = MessagingConfig::default();
        // ih tronque
        let d = AttachDesc {
            ih: [0u8; 20],
            mid: [0u8; 16],
            name: "f".into(),
            size: 1,
        };
        let mut m = BTreeMap::new();
        m.insert(b"ih".to_vec(), BValue::Bytes(vec![0u8; 19]));
        m.insert(b"mid".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        m.insert(b"name".to_vec(), BValue::Bytes(b"f".to_vec()));
        m.insert(b"size".to_vec(), BValue::Int(1));
        assert!(AttachDesc::decode_body(&BValue::Dict(m).encode(), &c).is_err());
        // size <= 0
        let d2 = AttachDesc {
            name: "f".into(),
            ..d.clone()
        };
        let mut m = BTreeMap::new();
        m.insert(b"ih".to_vec(), BValue::Bytes([0u8; 20].to_vec()));
        m.insert(b"mid".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        m.insert(b"name".to_vec(), BValue::Bytes(b"f".to_vec()));
        m.insert(b"size".to_vec(), BValue::Int(0));
        assert!(AttachDesc::decode_body(&BValue::Dict(m).encode(), &c).is_err());
        // name trop long
        let d3 = AttachDesc {
            name: "n".repeat(c.group_name_max_len + 1),
            ..d2
        };
        assert!(AttachDesc::decode_body(&d3.encode(), &c).is_err());
        // cle supplementaire
        let mut m = BTreeMap::new();
        m.insert(b"ih".to_vec(), BValue::Bytes([0u8; 20].to_vec()));
        m.insert(b"mid".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        m.insert(b"name".to_vec(), BValue::Bytes(b"f".to_vec()));
        m.insert(b"size".to_vec(), BValue::Int(1));
        m.insert(b"x".to_vec(), BValue::Int(1));
        assert!(AttachDesc::decode_body(&BValue::Dict(m).encode(), &c).is_err());
    }
}
