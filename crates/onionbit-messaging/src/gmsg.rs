// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps d'une trame `msg` **de groupe** (ADR-0019 §3) : le `mid`
//! applicatif (16 o) voyage dans le corps chiffre — la dedup
//! `(conv, author, mid)` survit aux re-emissions apres reouverture
//! de circuit, la ou la dedup `id` de trame est propre a chaque
//! emission.
//!
//! ```text
//! { "b": <payload applicatif>, "m": <mid 16 o> }
//! ```
//!
//! Le `msg` 1:1 garde son corps brut historique (pas d'enveloppe).

use std::collections::BTreeMap;

use onionbit_format::bencode::{decode, BValue};

use crate::error::MessagingError;

/// Taille du `mid` applicatif (= taille de l'`id` de trame).
pub const MID_LEN: usize = 16;

/// Encode le corps d'un `msg` de groupe : `{b: payload, m: mid}`.
pub fn encode_gmsg(mid: &[u8; MID_LEN], payload: &[u8]) -> Vec<u8> {
    let mut d = BTreeMap::new();
    d.insert(b"b".to_vec(), BValue::Bytes(payload.to_vec()));
    d.insert(b"m".to_vec(), BValue::Bytes(mid.to_vec()));
    BValue::Dict(d).encode()
}

/// Decode un corps de `msg` de groupe : `(mid, payload)` — strict
/// (exactement deux cles, `m` de 16 octets).
pub fn decode_gmsg(body: &[u8]) -> Result<([u8; MID_LEN], Vec<u8>), MessagingError> {
    let value = decode(body)?;
    let dict = value.as_dict().ok_or(MessagingError::Malformed(
        "gmsg : n'est pas un dictionnaire",
    ))?;
    if dict.len() != 2 {
        return Err(MessagingError::Malformed("gmsg : cles"));
    }
    let mid_b = dict
        .get(b"m".as_ref())
        .and_then(BValue::as_bytes)
        .ok_or(MessagingError::Malformed("gmsg : m absent"))?;
    let mid = <[u8; MID_LEN]>::try_from(mid_b)
        .map_err(|_| MessagingError::Malformed("gmsg : m taille"))?;
    let payload = dict
        .get(b"b".as_ref())
        .and_then(BValue::as_bytes)
        .ok_or(MessagingError::Malformed("gmsg : b absent"))?;
    Ok((mid, payload.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roundtrip + lecture d'un payload quelconque (non utf8).
    #[test]
    fn gmsg_roundtrip() {
        let mid = [7u8; 16];
        let payload = [0xff, 0x00, 0x41];
        let (m, b) = decode_gmsg(&encode_gmsg(&mid, &payload)).unwrap();
        assert_eq!(m, mid);
        assert_eq!(b, payload);
    }

    /// Corps hostiles : non-dict, cles supplementaires, `m`
    /// tronque, payload vide autorise.
    #[test]
    fn gmsg_corps_hostiles() {
        assert!(decode_gmsg(b"corps brut").is_err());
        let mut d = BTreeMap::new();
        d.insert(b"b".to_vec(), BValue::Bytes(vec![1]));
        d.insert(b"m".to_vec(), BValue::Bytes([7u8; 16].to_vec()));
        d.insert(b"x".to_vec(), BValue::Int(1));
        assert!(decode_gmsg(&BValue::Dict(d).encode()).is_err());
        let mut d = BTreeMap::new();
        d.insert(b"b".to_vec(), BValue::Bytes(vec![1]));
        d.insert(b"m".to_vec(), BValue::Bytes([7u8; 15].to_vec()));
        assert!(decode_gmsg(&BValue::Dict(d).encode()).is_err());
        // payload vide : accepte (un ping de groupe reste possible).
        let (m, b) = decode_gmsg(&encode_gmsg(&[1u8; 16], &[])).unwrap();
        assert!(b.is_empty() && m == [1u8; 16]);
    }
}
