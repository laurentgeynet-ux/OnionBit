// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps de la trame `hello` (ADR-0011, ADR-0019 §3).
//!
//! - **v1** : `pk_bin` seule (74 octets, format `to_bin` complet) —
//!   la premiere trame d'un inconnu declare son emetteur, verifiee
//!   ensuite par signature.
//! - **v2** : `pk_bin ‖ caps` (74 + 8 octets) — `caps` est le bitmap
//!   de capacites applicatives de la messagerie (aligne sur
//!   `CAP_MSG_V2` ext). Le corps reste borne par `max_body_len`,
//!   bien au-dela du besoin.

use onionbit_crypto::ipv8::keys::LIBNACL_PK_BIN_LEN;

use crate::error::MessagingError;

/// Taille du corps `hello` v1 (`pk_bin` seule, format `to_bin`).
pub const HELLO_V1_LEN: usize = LIBNACL_PK_BIN_LEN;
/// Taille du corps `hello` v2 (`pk_bin ‖ caps` big-endian).
pub const HELLO_V2_LEN: usize = LIBNACL_PK_BIN_LEN + 8;

/// Capacite applicative du `hello` v2 : groupes + pieces jointes
/// (ADR-0019). Meme valeur que `CAP_MSG_V2` ext — les deux annonces
/// restent coherentes (ext = transport, hello = applicatif).
pub const HELLO_CAP_GROUPS: u64 = 1 << 2;

/// Corps `hello` v1 : la `pk_bin` seule (comportement historique).
pub fn encode_hello_v1(pk_bin: &[u8]) -> Vec<u8> {
    pk_bin.to_vec()
}

/// Corps `hello` v2 : `pk_bin ‖ caps` (caps big-endian).
pub fn encode_hello_v2(pk_bin: &[u8], caps: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(pk_bin.len() + 8);
    body.extend_from_slice(pk_bin);
    body.extend_from_slice(&caps.to_be_bytes());
    body
}

/// Decode un corps `hello` : `pk_bin` (74 o) → `(pk, caps = 0)`
/// (v1) ; `pk_bin ‖ caps` (82 o) → `(pk, caps)` (v2). Toute autre
/// longueur est rejetee. La `pk` est rendue en octets bruts — la
/// reconstruction `LibNaClPublicKey` reste au service.
pub fn decode_hello(body: &[u8]) -> Result<(Vec<u8>, u64), MessagingError> {
    match body.len() {
        HELLO_V1_LEN => Ok((body.to_vec(), 0)),
        HELLO_V2_LEN => {
            let caps =
                u64::from_be_bytes(body[LIBNACL_PK_BIN_LEN..].try_into().unwrap_or_default());
            Ok((body[..LIBNACL_PK_BIN_LEN].to_vec(), caps))
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
        let pk = vec![7u8; LIBNACL_PK_BIN_LEN];
        let (pk1, caps1) = decode_hello(&encode_hello_v1(&pk)).unwrap();
        assert_eq!((pk1, caps1), (pk.clone(), 0));
        let (pk2, caps2) = decode_hello(&encode_hello_v2(&pk, HELLO_CAP_GROUPS)).unwrap();
        assert_eq!((pk2, caps2), (pk, HELLO_CAP_GROUPS));
    }

    /// Longueurs invalides rejetees.
    #[test]
    fn hello_tailles_invalides() {
        assert!(decode_hello(&[0u8; LIBNACL_PK_BIN_LEN - 1]).is_err());
        assert!(decode_hello(&[0u8; LIBNACL_PK_BIN_LEN + 1]).is_err());
        assert!(decode_hello(&[0u8; HELLO_V2_LEN + 1]).is_err());
        assert!(decode_hello(&[]).is_err());
    }
}
