// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps de la trame `hello` (ADR-0011, ADR-0019 §3).
//!
//! - **v1** : `pk` seule (32 octets) — la premiere trame d'un
//!   inconnu declare son emetteur, verifiee ensuite par signature.
//! - **v2** : `pk ‖ caps` (32 + 8 octets) — `caps` est le bitmap de
//!   capacites applicatives de la messagerie (bit 0 = groupes/
//!   attachements, aligne sur `CAP_MSG_V2` ext). Le corps reste
//!   borne par `max_body_len`, bien au-dela du besoin.

use crate::error::MessagingError;

/// Taille du corps `hello` v1 (cle publique seule).
pub const HELLO_V1_LEN: usize = 32;
/// Taille du corps `hello` v2 (`pk ‖ caps` big-endian).
pub const HELLO_V2_LEN: usize = 32 + 8;

/// Capacite applicative du `hello` v2 : groupes + pieces jointes
/// (ADR-0019). Meme valeur que `CAP_MSG_V2` ext — les deux annonces
/// restent coherentes (ext = transport, hello = applicatif).
pub const HELLO_CAP_GROUPS: u64 = 1 << 2;

/// Corps `hello` v1 : la `pk` seule (comportement historique).
pub fn encode_hello_v1(pk: &[u8; 32]) -> Vec<u8> {
    pk.to_vec()
}

/// Corps `hello` v2 : `pk ‖ caps` (caps big-endian).
pub fn encode_hello_v2(pk: &[u8; 32], caps: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(HELLO_V2_LEN);
    body.extend_from_slice(pk);
    body.extend_from_slice(&caps.to_be_bytes());
    body
}

/// Decode un corps `hello` : 32 octets → `(pk, caps = 0)` (v1) ;
/// 40 octets → `(pk, caps)` (v2). Toute autre longueur est rejetee.
pub fn decode_hello(body: &[u8]) -> Result<([u8; 32], u64), MessagingError> {
    match body.len() {
        HELLO_V1_LEN => {
            let mut pk = [0u8; 32];
            pk.copy_from_slice(body);
            Ok((pk, 0))
        }
        HELLO_V2_LEN => {
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&body[..32]);
            let caps = u64::from_be_bytes(body[32..].try_into().unwrap_or_default());
            Ok((pk, caps))
        }
        _ => Err(MessagingError::Malformed("corps hello : taille inattendue")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roundtrip v1 et v2 ; un corps v1 decode en caps = 0.
    #[test]
    fn hello_roundtrip_v1_v2() {
        let pk = [7u8; 32];
        let (pk1, caps1) = decode_hello(&encode_hello_v1(&pk)).unwrap();
        assert_eq!((pk1, caps1), (pk, 0));
        let (pk2, caps2) = decode_hello(&encode_hello_v2(&pk, HELLO_CAP_GROUPS)).unwrap();
        assert_eq!((pk2, caps2), (pk, HELLO_CAP_GROUPS));
    }

    /// Longueurs invalides rejetees.
    #[test]
    fn hello_tailles_invalides() {
        assert!(decode_hello(&[0u8; 31]).is_err());
        assert!(decode_hello(&[0u8; 33]).is_err());
        assert!(decode_hello(&[0u8; 41]).is_err());
        assert!(decode_hello(&[]).is_err());
    }
}
